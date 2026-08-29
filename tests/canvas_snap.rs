//! The 2D canvas's snapping preferences: the defaults a new project
//! opens on, the operators that change them, where they are kept, and
//! what they must stay out of.

use bevy::prelude::*;
use jackdaw::canvas_snap::{CanvasSnap, CanvasSnapKind};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::project_settings::{Section, load_section, settings_path};
use jackdaw_api::prelude::*;

mod util;

use util::OperatorResultExt as _;

fn snap(app: &App) -> CanvasSnap {
    *app.world().resource::<CanvasSnap>()
}

#[test]
fn the_snap_kinds_ship_with_the_defaults_the_canvas_expects() {
    let app = util::editor_test_app();
    let snap = snap(&app);

    let on = CanvasSnapKind::ALL
        .into_iter()
        .filter(|kind| snap.enabled(*kind))
        .map(CanvasSnapKind::id)
        .collect::<Vec<_>>();
    assert_eq!(
        on,
        vec![
            "pixel",
            "parent",
            "percent_lines",
            "sibling_sides",
            "sibling_centers",
            "guides",
        ],
        "every kind but the cross-tree one is on out of the box",
    );
    assert!(
        snap.show_rulers && snap.show_guides,
        "the rulers and the guides are drawn out of the box",
    );
    assert_eq!(
        CanvasSnapKind::ALL
            .into_iter()
            .filter_map(|kind| CanvasSnapKind::parse(kind.id()))
            .count(),
        CanvasSnapKind::ALL.len(),
        "every kind is reachable by the id a caller writes",
    );
}

#[test]
fn the_snap_operator_sets_flips_and_refuses_an_unknown_kind() {
    let mut app = util::editor_test_app();

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "other_nodes")
        .param("on", true)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();
    assert!(
        snap(&app).other_nodes,
        "a call naming a state puts the kind in that state",
    );

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "other_nodes")
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();
    assert!(
        !snap(&app).other_nodes,
        "a call naming no state flips the kind",
    );

    let before = snap(&app);
    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "nonesuch")
        .param("on", true)
        .call()
        .expect("canvas.snap dispatches")
        .assert_cancelled();
    assert_eq!(
        snap(&app),
        before,
        "a call naming no kind of the canvas's changes nothing",
    );

    app.world_mut()
        .operator("canvas.rulers")
        .param("on", false)
        .call()
        .expect("canvas.rulers dispatches")
        .assert_finished();
    app.world_mut()
        .operator("canvas.guides")
        .call()
        .expect("canvas.guides dispatches")
        .assert_finished();
    assert!(
        !snap(&app).show_rulers && !snap(&app).show_guides,
        "the view toggles set and flip the same way",
    );
}

/// A project directory of this test's own, holding `settings`.
fn project_with_settings(name: &str, settings: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jackdaw-canvas-snap-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".jackdaw")).expect("the project directory");
    std::fs::write(settings_path(&root), settings).expect("the settings the project opens with");
    root
}

fn open_project(app: &mut App, root: &std::path::Path) {
    app.world_mut().insert_resource(ProjectRoot {
        root: root.to_path_buf(),
        config: ProjectConfig::default(),
    });
    app.update();
}

#[test]
fn the_snap_kinds_survive_a_project_reopen_beside_the_build_settings() {
    let root = project_with_settings("reopen", "{\n  \"auto_build\": true\n}");
    let mut app = util::editor_test_app();
    open_project(&mut app, &root);

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "other_nodes")
        .param("on", true)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();

    let written = std::fs::read_to_string(settings_path(&root)).expect("the settings were written");
    let document: serde_json::Value =
        serde_json::from_str(&written).expect("the settings are still JSON");
    assert_eq!(
        document.get("auto_build"),
        Some(&serde_json::Value::Bool(true)),
        "the settings beside the canvas's are still there: {written}",
    );

    let reopened: CanvasSnap = load_section(&root, Section::Key("canvas"));
    assert_eq!(
        reopened,
        snap(&app),
        "a project reopened reads back what the editor last wrote",
    );
    assert!(
        reopened.other_nodes,
        "including the kind the user turned on",
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_snap_kinds_stay_out_of_undo_snapshots() {
    let mut app = util::editor_test_app();
    let before = util::snapshot(&mut app);

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "parent")
        .param("on", false)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();

    let after = util::snapshot(&mut app);
    assert!(
        before.equals(&*after),
        "a snap preference is not part of the scene an undo restores",
    );

    before.apply(app.world_mut());
    assert!(
        !snap(&app).parent,
        "undoing back past the change leaves the preference where the user put it",
    );
}
