//! The LOD bar: dragging a divider moves a switch as one undo entry, a
//! placement's edits land in its own override, and clicking a segment forces
//! that level in the viewport.

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{PrimaryWindow, WindowResolution};
use jackdaw::lod_bar::{BarTarget, LodBar, LodBarDivider, LodBarSegment, spawn_bar};
use jackdaw::test_input::SyntheticInput;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::model_import::{
    LevelShow, LodImportSource, ModelLevels, ModelLod, ModelLodIndex, ModelLodLevel,
};
use jackdaw_scene_types::{GltfSource, LodFade, LodOverride};

use crate::util;
use crate::util::OperatorResultExt as _;

const TREE: &str = "models/tree.gltf";

fn tree_lod() -> ModelLod {
    ModelLod {
        version: ModelLod::VERSION,
        source: LodImportSource::SiblingFiles,
        size: 2.0,
        fade: LodFade::Snap,
        levels: [0.5, 0.25, 0.1]
            .into_iter()
            .enumerate()
            .map(|(index, screen_height)| ModelLodLevel {
                show: match index {
                    0 => LevelShow::Model,
                    _ => LevelShow::File(format!("tree_LOD{index}.gltf")),
                },
                screen_height,
            })
            .collect(),
    }
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

/// An editor whose window holds a 600 px wide strip, with the tree's
/// settings read.
fn editor() -> (App, Entity, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    util::fixed_frame_clock(&mut app);
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows.single_mut(app.world_mut()).expect("a window");
        window.resolution = WindowResolution::new(1280, 720);
    }
    app.world_mut()
        .resource_mut::<ModelLodIndex>()
        .set(TREE, Some(tree_lod()));
    let strip = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                left: px(100.0),
                top: px(100.0),
                width: px(600.0),
                height: px(60.0),
                ..default()
            },
            GlobalZIndex(1000),
        ))
        .id();
    settle(&mut app);
    (app, strip, dir)
}

/// The middle of `node` in window logical pixels.
fn centre(app: &App, node: Entity) -> Vec2 {
    let computed = app.world().get::<ComputedNode>(node).expect("laid out");
    let placed = app.world().get::<UiGlobalTransform>(node).expect("placed");
    placed.translation * computed.inverse_scale_factor()
}

fn pointer(app: &mut App, at: Vec2, action: &str) {
    app.world_mut()
        .operator("input.pointer")
        .param("x", f64::from(at.x))
        .param("y", f64::from(at.y))
        .param("action", action.to_string())
        .call()
        .expect("input.pointer dispatches")
        .assert_finished();
    for _ in 0..600 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    assert!(
        app.world().resource::<SyntheticInput>().is_idle(),
        "the gesture drained"
    );
    settle(app);
}

fn find<T: Component + Copy>(app: &mut App, pick: impl Fn(&T) -> bool) -> Entity {
    app.world_mut()
        .query::<(Entity, &T)>()
        .iter(app.world())
        .find(|(_, item)| pick(item))
        .map(|(entity, _)| entity)
        .expect("the bar part")
}

fn heights(app: &App) -> Vec<f32> {
    app.world()
        .resource::<ModelLodIndex>()
        .get(TREE)
        .expect("settings")
        .levels
        .iter()
        .map(|level| level.screen_height)
        .collect()
}

fn history(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn dragging_a_divider_moves_its_switch_as_one_undo_entry() {
    let (mut app, strip, _dir) = editor();
    spawn_bar(app.world_mut(), strip, BarTarget::Model(TREE.into())).expect("a bar");
    settle(&mut app);
    let divider = find::<LodBarDivider>(&mut app, |divider| divider.level == 0);
    let from = centre(&app, divider);
    let before = history(&app);

    pointer(&mut app, from, "move");
    pointer(&mut app, from + Vec2::new(60.0, 0.0), "drag_to");

    assert_eq!(history(&app), before + 1);
    let moved = heights(&app);
    assert!((moved[0] - 0.4).abs() < 0.02, "{moved:?}");
    assert_eq!(moved[1..], [0.25, 0.1]);
    assert!(
        app.world()
            .resource::<jackdaw::model_lod::UnsavedModelSettings>()
            .0
            .contains(TREE)
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(&mut app);
    assert_eq!(heights(&app), [0.5, 0.25, 0.1]);
}

#[test]
fn clicking_a_segment_forces_its_level_and_clicking_again_lets_go() {
    let (mut app, strip, _dir) = editor();
    spawn_bar(app.world_mut(), strip, BarTarget::Model(TREE.into())).expect("a bar");
    settle(&mut app);
    let segment = find::<LodBarSegment>(&mut app, |segment| segment.level == 1);
    let at = centre(&app, segment);

    pointer(&mut app, at, "click");
    assert_eq!(
        app.world().resource::<jackdaw_runtime::ForcedLod>().0,
        Some(1)
    );

    pointer(&mut app, at, "click");
    assert_eq!(app.world().resource::<jackdaw_runtime::ForcedLod>().0, None);
}

#[test]
fn a_placements_drag_writes_its_override_and_leaves_the_model_alone() {
    let (mut app, strip, _dir) = editor();
    let placed = app
        .world_mut()
        .spawn((
            GltfSource {
                path: TREE.into(),
                scene_index: 0,
            },
            Transform::default(),
        ))
        .id();
    settle(&mut app);
    assert!(app.world().get::<ModelLevels>(placed).is_some());
    spawn_bar(app.world_mut(), strip, BarTarget::Placement(placed)).expect("a bar");
    settle(&mut app);
    let divider = find::<LodBarDivider>(&mut app, |divider| divider.level == 1);
    let from = centre(&app, divider);
    let before = history(&app);

    pointer(&mut app, from, "move");
    pointer(&mut app, from + Vec2::new(30.0, 0.0), "drag_to");

    assert_eq!(history(&app), before + 1);
    let held = app
        .world()
        .get::<LodOverride>(placed)
        .and_then(|held| held.screen_heights.clone())
        .expect("an override");
    assert!((held[1] - 0.2).abs() < 0.02, "{held:?}");
    assert_eq!(heights(&app), [0.5, 0.25, 0.1]);

    app.world_mut()
        .operator("lod.revert_override")
        .param("entity", placed)
        .param("field", "screen_heights".to_string())
        .call()
        .expect("lod.revert_override dispatches")
        .assert_finished();
    settle(&mut app);
    assert!(app.world().get::<LodOverride>(placed).is_none());
    assert_eq!(history(&app), before + 2);
    let shown: Vec<Vec<f32>> = app
        .world_mut()
        .query::<&LodBar>()
        .iter(app.world())
        .map(|bar| bar.heights.clone())
        .collect();
    assert_eq!(shown, [vec![0.5, 0.25, 0.1]]);
}

#[test]
fn the_lod_colour_view_tints_parts_without_touching_the_document() {
    let (mut app, _strip, _dir) = editor();
    let mut models = app
        .world_mut()
        .resource_mut::<jackdaw_scene_types::model_parts::ModelParts>();
    for path in [TREE, "models/tree_LOD1.gltf", "models/tree_LOD2.gltf"] {
        models.insert(
            path,
            jackdaw_scene_types::model_parts::FlatModel {
                parts: vec![jackdaw_scene_types::model_parts::ModelPart {
                    mesh: Handle::default(),
                    material: Handle::default(),
                    material_name: None,
                    local: Transform::IDENTITY,
                }],
                bounds: bevy::camera::primitives::Aabb::from_min_max(Vec3::ZERO, Vec3::ONE),
                needs_instance: false,
            },
        );
    }
    app.world_mut().spawn((
        Camera3d::default(),
        jackdaw::viewport::MainViewportCamera,
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let placed = app
        .world_mut()
        .spawn((
            GltfSource {
                path: TREE.into(),
                scene_index: 0,
            },
            Transform::default(),
        ))
        .id();
    settle(&mut app);
    let own: Vec<AssetId<StandardMaterial>> = parts_materials(&mut app);
    assert!(!own.is_empty(), "a level is in");

    app.world_mut()
        .operator("view.toggle_lod_colors")
        .call()
        .expect("view.toggle_lod_colors dispatches")
        .assert_finished();
    settle(&mut app);
    assert_ne!(parts_materials(&mut app), own);
    assert!(app.world().get::<LodOverride>(placed).is_none());

    app.world_mut()
        .operator("view.toggle_lod_colors")
        .call()
        .expect("view.toggle_lod_colors dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(parts_materials(&mut app), own);
}

fn parts_materials(app: &mut App) -> Vec<AssetId<StandardMaterial>> {
    app.world_mut()
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<jackdaw_runtime::LodPart>>()
        .iter(app.world())
        .map(|material| material.0.id())
        .collect()
}
