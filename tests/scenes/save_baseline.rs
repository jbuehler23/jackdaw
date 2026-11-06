//! What a save writes for a scene with a prefab instance, a terrain and a runtime
//! material, compared with a recorded file.

use crate::util::{self, OperatorResultExt as _};

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause;

pub(super) const TERRAIN_FILE: &str = "zone.terrain-0.jdterrain";

/// An editor with the fixture's scene open and the parts only a live editor holds
/// put in place.
pub(super) fn fixture_project() -> (App, tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets folder");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/save_baseline/assets");
    for name in ["crate.bsn", "zone.bsn"] {
        std::fs::copy(fixture.join(name), assets.join(name)).expect("fixture copied");
    }

    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    run_op_clause(app.world_mut(), "scene.open path=assets/zone.bsn")
        .expect("the scene opens")
        .assert_finished();
    app.update();

    app.world_mut()
        .resource_mut::<jackdaw::terrain_mode::TerrainDataStore>()
        .insert(
            TERRAIN_FILE.to_string(),
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 4,
                heights: (0..16).map(|cell| cell as f32 * 0.25).collect(),
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
        );

    let material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(0.25, 0.5, 0.75),
            ..default()
        });
    let mut brush = jackdaw_scene_types::Brush::cuboid(1.0, 1.0, 1.0);
    brush.faces[0].material = material;
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Painted"),
            Transform::default(),
            Visibility::Inherited,
            brush,
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();

    let scene = assets.join("zone.bsn");
    (app, tmp, scene)
}

/// One entity's patches, each as its lines.
type EntityBlock = Vec<String>;

/// The file's entities, in the order it holds them, each a sorted list of its patches
/// with the version stamp left out and the node ids minted on opening blanked. Only
/// the order of patches within an entity is ignored, since that varies between builds.
pub(super) fn entity_blocks(text: &str) -> Vec<EntityBlock> {
    let mut blocks = vec![Vec::new()];
    let mut open = 0;
    for line in text
        .lines()
        .filter(|line| !line.starts_with(jackdaw_bsn::migrate::STAMP_MARKER))
    {
        if open == 0 {
            let wrapper = line.ends_with("Children [") && !line.starts_with(' ') || line == "]";
            if wrapper {
                continue;
            }
            if line.trim() == "--" {
                blocks.push(Vec::new());
                continue;
            }
        }
        let line = blank_node_id(line);
        let block: &mut EntityBlock = blocks.last_mut().expect("starts with one block");
        match block.last_mut() {
            Some(patch) if open > 0 => {
                patch.push('\n');
                patch.push_str(&line);
            }
            _ => block.push(line.clone()),
        }
        open += brackets_opened(&line);
    }
    for block in &mut blocks {
        block.sort_unstable();
    }
    blocks
}

/// How many more brackets `line` opens than it closes, outside its strings.
fn brackets_opened(line: &str) -> i32 {
    let mut opened = 0;
    let mut in_string = false;
    let mut escaped = false;
    for byte in line.bytes() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' | b'[' | b'(' if !in_string => opened += 1,
            b'}' | b']' | b')' if !in_string => opened -= 1,
            _ => {}
        }
    }
    opened
}

fn blank_node_id(line: &str) -> String {
    const NODE_ID: &str = "node_id::SceneNodeId(";
    match line.find(NODE_ID) {
        Some(at) => format!("{}{NODE_ID}_)", &line[..at]),
        None => line.to_string(),
    }
}

pub(super) fn recorded() -> String {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/save_baseline/saved.bsn");
    std::fs::read_to_string(file).expect("the recorded file")
}

#[test]
fn a_blocking_save_writes_the_recorded_file() {
    let (mut app, _tmp, scene) = fixture_project();
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let saved = std::fs::read_to_string(&scene).expect("the file");
    assert_eq!(entity_blocks(&saved), entity_blocks(&recorded()));
}

const TWO_NODES: &str = "\
bevy_ecs::hierarchy::Children [
    #A
    x::Transform {
        y: 1.0,
    }
    x::Tag
    --
    #B
    x::Other
]
";

#[test]
fn entities_are_compared_whole_and_in_order() {
    let reordered_patches = "\
bevy_ecs::hierarchy::Children [
    #A
    x::Tag
    x::Transform {
        y: 1.0,
    }
    --
    #B
    x::Other
]
";
    let moved_between_entities = "\
bevy_ecs::hierarchy::Children [
    #A
    x::Transform {
        y: 1.0,
    }
    --
    #B
    x::Tag
    x::Other
]
";
    let moved_field = "\
bevy_ecs::hierarchy::Children [
    #A
    x::Transform
    x::Tag {
        y: 1.0,
    }
    --
    #B
    x::Other
]
";
    let reordered_entities = "\
bevy_ecs::hierarchy::Children [
    #B
    x::Other
    --
    #A
    x::Transform {
        y: 1.0,
    }
    x::Tag
]
";
    assert_eq!(entity_blocks(TWO_NODES), entity_blocks(reordered_patches));
    assert_ne!(
        entity_blocks(TWO_NODES),
        entity_blocks(moved_between_entities)
    );
    assert_ne!(entity_blocks(TWO_NODES), entity_blocks(moved_field));
    assert_ne!(entity_blocks(TWO_NODES), entity_blocks(reordered_entities));
}
