//! A generated scene the size of a large hand-built level saves through the
//! editor, parses back with every node it was given, and opens again with its
//! brushes, faces and materials.

use crate::util;

use std::path::Path;
use std::time::Instant;

use bevy::prelude::*;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::testing::large_scene::{LargeScene, write_large_scene};
use jackdaw_bsn::SceneBsnAst;
use jackdaw_scene_types::Brush;

const BRUSH: &str = "jackdaw_scene_types::types::Brush";
const IS_A: &str = "jackdaw::prefab::components::IsA";

/// How many nodes sit under `node`, itself included, how many of them are
/// brushes, and how deep the deepest goes. A prefab instance counts as one
/// node: what it lists below itself mirrors the prefab.
fn walk(ast: &SceneBsnAst, node: Entity, depth: usize, tally: &mut (usize, usize, usize)) {
    tally.0 += 1;
    tally.2 = tally.2.max(depth);
    if ast.find_patch_by_type_path(node, BRUSH).is_some() {
        tally.1 += 1;
    }
    if ast.find_patch_by_type_path(node, IS_A).is_some() {
        return;
    }
    for child in ast.get_children_ast(node) {
        walk(ast, child, depth + 1, tally);
    }
}

fn open(project: &Path, scene: &LargeScene) -> App {
    let mut app = util::editor_test_app_reading(&project.join("assets"));
    app.insert_resource(ProjectRoot::new(
        project.to_path_buf(),
        ProjectConfig::default(),
    ));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene.path);
    util::settle_scene_load(&mut app);
    app
}

#[test]
#[expect(clippy::print_stderr, reason = "reports the scene's size and timings")]
fn a_generated_large_scene_saves_parses_and_opens_at_full_size() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let started = Instant::now();
    let scene = write_large_scene(tmp.path()).expect("the scene is generated");
    let generated = started.elapsed();

    assert_eq!(scene.brushes, 761, "{scene:?}");
    assert!(
        (18_000..=24_000).contains(&scene.faces),
        "tens of thousands of faces: {scene:?}"
    );
    assert_eq!(scene.materials, 43, "every material is used: {scene:?}");
    assert!(scene.groups >= 40, "{scene:?}");
    assert!(scene.prefab_instances >= 2, "{scene:?}");

    let started = Instant::now();
    let text = jackdaw_bsn::read_document_text(&scene.path).expect("the scene reads");
    let ast = jackdaw_bsn::parse_emitted(&text).expect("the saved scene parses");
    let parsed = started.elapsed();
    let mut tally = (0, 0, 0);
    for root in ast.roots.clone() {
        walk(&ast, root, 1, &mut tally);
    }
    let (nodes, brush_nodes, depth) = tally;
    assert_eq!(nodes, scene.nodes(), "every node reads back");
    assert_eq!(
        brush_nodes, scene.brushes,
        "every authored brush reads back"
    );
    assert!(depth >= 5, "groups nest several levels deep, got {depth}");

    let started = Instant::now();
    let mut app = open(tmp.path(), &scene);
    let opened = started.elapsed();

    let mut brushes = app.world_mut().query::<&Brush>();
    let (brush_count, faces, materials) = {
        let world = app.world();
        let stored = world.resource::<Assets<StandardMaterial>>();
        let mut materials = std::collections::HashSet::new();
        let mut faces = 0;
        let mut count = 0;
        for brush in brushes.iter(world) {
            count += 1;
            faces += brush.faces.len();
            for face in &brush.faces {
                if stored.contains(&face.material) {
                    materials.insert(face.material.id());
                }
            }
        }
        (count, faces, materials.len())
    };
    assert_eq!(
        brush_count,
        scene.brushes + scene.prefab_instances * scene.prefab_brushes,
        "every brush spawns, the prefab instances' included"
    );
    assert!(
        faces >= scene.faces,
        "{faces} faces spawned of {}",
        scene.faces
    );
    assert_eq!(
        materials, scene.materials,
        "every face finds its material file"
    );

    let spawned = app
        .world_mut()
        .query_filtered::<(), Or<(With<jackdaw_bsn::AstNodeRef>, With<Mesh3d>)>>()
        .iter(app.world())
        .count();
    let bytes = std::fs::metadata(&scene.path).map_or(0, |meta| meta.len());
    eprintln!(
        "large scene: {nodes} nodes, depth {depth}, {} authored faces; after opening \
         {brush_count} brushes, {faces} faces, {materials} materials, {spawned} scene and \
         mesh entities; {bytes} bytes; generated in {generated:?}, parsed in {parsed:?}, \
         opened in {opened:?}",
        scene.faces
    );
}

/// Writes the scene into the project folder `JACKDAW_LARGE_SCENE_OUT` names, for
/// opening by hand: `JACKDAW_LARGE_SCENE_OUT=<folder> cargo test --test editor
/// large_scene -- --ignored`.
#[test]
#[ignore = "writes into the folder JACKDAW_LARGE_SCENE_OUT names"]
#[expect(clippy::print_stderr, reason = "reports where the scene went")]
fn writes_the_large_scene_into_a_project_folder() {
    let out = std::env::var_os("JACKDAW_LARGE_SCENE_OUT")
        .expect("JACKDAW_LARGE_SCENE_OUT names the project folder to write into");
    let out = Path::new(&out).join("assets");
    let tmp = tempfile::tempdir().expect("a temp directory");
    let scene = write_large_scene(tmp.path()).expect("the scene is generated");
    let text = jackdaw_bsn::read_document_text(&scene.path).expect("the scene reads");
    let ast = jackdaw_bsn::parse_emitted(&text).expect("the saved scene parses");
    let mut tally = (0, 0, 0);
    for root in ast.roots.clone() {
        walk(&ast, root, 1, &mut tally);
    }
    assert_eq!(tally.0, scene.nodes(), "every node reads back: {scene:?}");
    let generated = scene
        .path
        .parent()
        .expect("the scene sits in the assets folder");
    let taken: Vec<_> = files_under(generated)
        .into_iter()
        .filter_map(|file| file.strip_prefix(generated).ok().map(|file| out.join(file)))
        .filter(|target| target.exists())
        .collect();
    assert!(
        taken.is_empty(),
        "would overwrite files already in the project: {taken:?}"
    );
    copy_tree(generated, &out);
    eprintln!(
        "wrote {} into {}: {scene:?}",
        scene.path.display(),
        out.display()
    );
}

/// Every file under `dir`, at any depth.
fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).expect("the generated folder reads") {
        let entry = entry.expect("a directory entry");
        if entry.file_type().expect("a file type").is_dir() {
            files.extend(files_under(&entry.path()));
        } else {
            files.push(entry.path());
        }
    }
    files
}

/// Copy every file under `from` into `to`, keeping the layout.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("the target folder is made");
    for entry in std::fs::read_dir(from).expect("the generated folder reads") {
        let entry = entry.expect("a directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a generated file copies");
        }
    }
}
