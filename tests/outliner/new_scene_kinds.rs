//! Making a scene of each kind: what `scene.new kind=...` puts in the document,
//! what it names it, and where it leaves the workspace.

use crate::util;

use bevy::prelude::*;
use jackdaw::scenes::operators::{SceneKind, scene_new_configured};
use jackdaw::selection::Selection;
use jackdaw::ui_palette::UI_SCENE_ROOT_NAME;
use jackdaw::viewport::VIEWPORT_WINDOW_ID;
use jackdaw::viewport_host::{ViewportMode, ViewportModeIntent};
use jackdaw_api_internal::OperatorWorldExt;
use jackdaw_panels::{
    area::DockAreaStyle,
    tree::{DockLeaf, DockTree, NodeId},
};
use jackdaw_scene_types::{Scene2dRoot, UiSceneRoot};

/// Every name in the live scene document, in no particular order.
fn scene_names(app: &mut App) -> Vec<String> {
    let mut query = app
        .world_mut()
        .query_filtered::<&Name, With<jackdaw_scene_types::SceneNodeId>>();
    query
        .iter(app.world())
        .map(|name| name.as_str().to_string())
        .collect()
}

/// A dock whose one leaf holds `windows`, with the first in front, so a
/// focus change reads as a change rather than as the starting state.
fn dock_leaf(app: &mut App, windows: &[&str]) -> NodeId {
    app.init_resource::<DockTree>();
    let mut tree = app.world_mut().resource_mut::<DockTree>();
    tree.set_root_leaf(
        DockLeaf::new("center", DockAreaStyle::default())
            .with_windows(windows.iter().copied().map(String::from).collect()),
    )
}

/// What the viewport panels of the active tab are being asked to show.
fn intent(app: &App) -> ViewportModeIntent {
    *app.world().resource::<ViewportModeIntent>()
}

/// Put the viewport in a mode the scene's kind did not ask for, so a later
/// activation's write reads as a change.
fn set_intent(app: &mut App, mode: ViewportMode) {
    *app.world_mut().resource_mut::<ViewportModeIntent>() = ViewportModeIntent {
        mode,
        chosen: false,
    };
}

/// The window whose tab is in front of `leaf`.
fn active_window(app: &App, leaf: NodeId) -> Option<String> {
    let tree = app.world().resource::<DockTree>();
    let leaf = tree.get(leaf)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
}

/// A UI document has nothing to light, and whatever is in the document at the
/// end of this is what a save writes.
#[test]
fn a_new_ui_scene_seeds_the_ui_root_and_nothing_else() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), SceneKind::Ui, None);
    app.update();

    let names = scene_names(&mut app);
    assert_eq!(
        names,
        vec![UI_SCENE_ROOT_NAME.to_string()],
        "a UI scene starts with its root alone: {names:?}",
    );

    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        0,
        "no light is seeded into a document that has nothing to light",
    );
}

/// The other half of the same branch: an ordinary scene still gets its light.
#[test]
fn a_new_3d_scene_still_seeds_its_light_and_no_ui_root() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), SceneKind::ThreeD, None);
    app.update();

    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        1,
        "an empty 3D scene is black without it",
    );
    let mut roots = app.world_mut().query::<&UiSceneRoot>();
    assert_eq!(
        roots.iter(app.world()).count(),
        0,
        "and a 3D scene is not a UI scene",
    );
}

/// An operator clause is text with no quoting, so a root called `UI Root` is
/// unreachable from every scripted path the editor has.
#[test]
fn the_seeded_root_is_named_so_a_clause_can_address_it() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), SceneKind::Ui, None);
    app.update();

    assert_eq!(UI_SCENE_ROOT_NAME, "UiRoot");
    assert!(
        !UI_SCENE_ROOT_NAME.contains(' '),
        "a clause value cannot contain a space, so neither can this name",
    );

    let mut query = app.world_mut().query::<(Entity, &Name, &UiSceneRoot)>();
    let (root, name, _) = query
        .iter(app.world())
        .next()
        .map(|(entity, name, root)| (entity, name.as_str().to_string(), root))
        .expect("the UI scene has a root");
    assert_eq!(name, UI_SCENE_ROOT_NAME);
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(root),
        "and it is selected, so the first Add lands inside it",
    );
}

/// The load and tab-swap paths already front the canvas panel; creation did not.
/// One panel shows both, so what comes forward is that panel, in 2D.
#[test]
fn creating_a_ui_scene_brings_the_viewport_forward_on_its_canvas() {
    let mut app = util::editor_test_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_WINDOW_ID]);
    app.update();
    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some("jackdaw.outliner"),
        "the fixture starts with the viewport behind another tab",
    );

    scene_new_configured(app.world_mut(), SceneKind::Ui, None);
    app.update();

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_WINDOW_ID),
        "a new UI scene fronts the panel it is authored in",
    );
    assert_eq!(
        intent(&app),
        ViewportModeIntent {
            mode: ViewportMode::TwoD,
            chosen: false,
        },
        "and asks it for the canvas, on the scene's behalf rather than the \
         user's: a switch afterwards is the user's and is remembered",
    );
}

/// And only for a UI scene. The mode still follows the kind, so a 3D scene made
/// from a UI tab does not open on the canvas the UI scene left behind.
#[test]
fn creating_a_3d_scene_leaves_the_fronted_tab_alone() {
    let mut app = util::editor_test_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_WINDOW_ID]);
    app.update();
    // As a UI tab would leave it: on the canvas, and not by the user's hand.
    set_intent(&mut app, ViewportMode::TwoD);

    scene_new_configured(app.world_mut(), SceneKind::ThreeD, None);
    app.update();

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some("jackdaw.outliner"),
        "nothing about a 3D scene asks for a panel to come forward",
    );
    assert_eq!(
        intent(&app),
        ViewportModeIntent {
            mode: ViewportMode::ThreeD,
            chosen: false,
        },
        "but the viewport it is authored in is the world one",
    );
}

/// A 2D scene is a world scene with nothing in it yet: the light a 3D scene needs
/// lights nothing a sprite draws, and the rest is furniture a save would write.
#[test]
fn a_new_2d_scene_seeds_its_root_and_no_3d_furniture() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), SceneKind::TwoD, None);
    app.update();

    let names = scene_names(&mut app);
    assert_eq!(
        names,
        vec![jackdaw::entity_ops::SCENE_2D_ROOT_NAME.to_string()],
        "a 2D scene starts with its root alone: {names:?}",
    );
    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        0,
        "a sprite is not lit, so a 2D scene seeds no light",
    );
    let mut ui_roots = app.world_mut().query::<&UiSceneRoot>();
    assert_eq!(
        ui_roots.iter(app.world()).count(),
        0,
        "and a 2D world scene is not a UI screen",
    );
    let mut roots = app.world_mut().query::<&Scene2dRoot>();
    assert_eq!(
        roots.iter(app.world()).count(),
        1,
        "the marker is what says which kind this is",
    );
}

/// A 2D world scene is drawn flat, so it is authored on the canvas like a
/// UI screen, and wants the same panel in front.
#[test]
fn creating_a_2d_scene_brings_the_viewport_forward_on_its_canvas() {
    let mut app = util::editor_test_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_WINDOW_ID]);
    app.update();

    scene_new_configured(app.world_mut(), SceneKind::TwoD, None);
    app.update();

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_WINDOW_ID),
        "a scene drawn flat is authored in the panel that can show it flat",
    );
    assert_eq!(
        intent(&app),
        ViewportModeIntent {
            mode: ViewportMode::TwoD,
            chosen: false,
        },
    );
}

/// The kind is a component in the document, so reopening the file finds it;
/// editor state would not survive the round trip.
#[test]
fn a_saved_scenes_kind_survives_a_reopen() {
    for kind in [SceneKind::TwoD, SceneKind::Ui] {
        let mut app = util::editor_test_app();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("kept.bsn");

        scene_new_configured(app.world_mut(), kind, Some(&path));
        app.update();
        assert!(
            jackdaw::scene_io::save_scene(app.world_mut()),
            "the scene must save before it can be reopened",
        );
        app.update();

        let written = std::fs::read_to_string(&path).expect("read the saved scene");
        let marker = match kind {
            SceneKind::TwoD => "Scene2dRoot",
            SceneKind::Ui => "UiSceneRoot",
            SceneKind::ThreeD => unreachable!("3D is the kind with no marker"),
        };
        assert!(
            written.contains(marker),
            "the kind is written into the document, not held in the editor:\n{written}",
        );

        // A fresh editor: the tab that wrote the file is still open in this one,
        // and reopening its own path just switches to it.
        let mut app = util::editor_test_app();
        // The mode a reopen lands in has to come from the document too: the
        // operator parameter that made this scene is long gone.
        set_intent(&mut app, ViewportMode::ThreeD);
        jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
        app.update();

        assert_eq!(
            intent(&app),
            ViewportModeIntent {
                mode: ViewportMode::TwoD,
                chosen: false,
            },
            "a reopened flat document comes back on the canvas it was drawn on",
        );

        match kind {
            SceneKind::TwoD => {
                let mut roots = app.world_mut().query::<&Scene2dRoot>();
                assert_eq!(
                    roots.iter(app.world()).count(),
                    1,
                    "the reopened document is still a 2D scene",
                );
            }
            SceneKind::Ui => {
                let mut roots = app.world_mut().query::<&UiSceneRoot>();
                assert_eq!(
                    roots.iter(app.world()).count(),
                    1,
                    "the reopened document is still a UI scene",
                );
            }
            SceneKind::ThreeD => unreachable!("3D is the kind with no marker"),
        }
    }
}

/// The row a user clicks, dispatched the way a click dispatches it: the action
/// string parsed into the operator call feathers attaches, then through the
/// editor's button observer. Asserting on the rows alone would not catch a
/// clause that never reaches the operator.
fn click_new_scene_row(app: &mut App, label: &str) {
    use bevy::ui_widgets::Activate;
    use jackdaw_feathers::button::ButtonOperatorCall;

    let rows = jackdaw::new_scene_rows();
    let (action, _) = rows
        .iter()
        .find(|(_, row_label)| row_label == label)
        .unwrap_or_else(|| panic!("the New group offers `{label}`: {rows:?}"));
    let call =
        ButtonOperatorCall::try_from(action.as_str()).expect("the row is an operator action");

    let button = app.world_mut().spawn(call).id();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    app.update();
}

#[test]
fn clicking_the_ui_row_makes_a_ui_scene() {
    let mut app = util::editor_test_app();

    click_new_scene_row(&mut app, "UI");

    let names = scene_names(&mut app);
    assert_eq!(
        names,
        vec![UI_SCENE_ROOT_NAME.to_string()],
        "the row's clause has to reach the operator: {names:?}",
    );
    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        0,
        "a UI scene made from the menu is the same UI scene",
    );
}

#[test]
fn clicking_the_2d_row_makes_a_2d_scene() {
    let mut app = util::editor_test_app();

    click_new_scene_row(&mut app, "2D");

    let mut roots = app.world_mut().query::<&Scene2dRoot>();
    assert_eq!(roots.iter(app.world()).count(), 1);
    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        0,
        "no 3D furniture in a 2D scene, however it was made",
    );
}

#[test]
fn clicking_the_3d_row_makes_a_3d_scene() {
    let mut app = util::editor_test_app();

    click_new_scene_row(&mut app, "3D");

    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        1,
        "an empty 3D scene is black without it",
    );
    let mut ui_roots = app.world_mut().query::<&UiSceneRoot>();
    assert_eq!(ui_roots.iter(app.world()).count(), 0);
    let mut roots = app.world_mut().query::<&Scene2dRoot>();
    assert_eq!(roots.iter(app.world()).count(), 0);
}

/// `ui=true` is what scripted runs and older keymaps spell, and it keeps meaning
/// `kind=ui` for one release.
#[test]
fn the_ui_true_alias_still_makes_a_ui_scene() {
    let mut app = util::editor_test_app();

    let _ = app
        .world_mut()
        .operator("scene.new")
        .param("ui", true)
        .call()
        .expect("scene.new dispatch");
    app.update();
    app.update();

    let names = scene_names(&mut app);
    assert_eq!(
        names,
        vec![UI_SCENE_ROOT_NAME.to_string()],
        "the old spelling still asks for a UI scene: {names:?}",
    );
}

/// And the alias's string form: an action string carries `ui=true` as text.
#[test]
fn the_alias_reads_the_same_from_a_text_clause() {
    let mut app = util::editor_test_app();

    let _ = app
        .world_mut()
        .operator("scene.new")
        .param("ui", "true".to_string())
        .call()
        .expect("scene.new dispatch");
    app.update();
    app.update();

    let mut roots = app.world_mut().query::<&UiSceneRoot>();
    assert_eq!(
        roots.iter(app.world()).count(),
        1,
        "a clause is text; a parameter read that ignores text ignores the user",
    );
}
