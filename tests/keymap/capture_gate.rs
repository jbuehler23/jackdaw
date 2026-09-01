//! While the keybind dialog is recording, a key press names a chord and
//! does nothing else.
//!
//! The dispatcher's own gate covers every operator reached through the
//! keymap. What is pinned here is the rest: the panels that read the
//! keyboard directly, because their keys are how a surface is used rather
//! than commands with chords of their own. Each has to stand down on its
//! own, and each used to act on the press that was naming it.

use crate::util;

use bevy::prelude::*;
use jackdaw_api_internal::keymap::KeymapCapture;
use jackdaw_widgets::tree_view::{
    TreeFocused, TreeNode, TreeNodeExpanded, TreeRowContent, TreeView,
};

/// A tree of two rows and the navigation system, with nothing else in the
/// way.
fn tree_app() -> (App, Vec<Entity>) {
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<TreeFocused>()
        .init_resource::<KeymapCapture>()
        .init_resource::<bevy::input_focus::InputFocus>()
        .add_systems(
            Update,
            jackdaw_feathers::tree_view::tree_keyboard_navigation,
        );

    let tree = app.world_mut().spawn((TreeView, Node::default())).id();
    let rows: Vec<Entity> = (0..2)
        .map(|_| {
            let source = app.world_mut().spawn_empty().id();
            let content = app
                .world_mut()
                .spawn((TreeRowContent, Node::default()))
                .id();
            let row = app
                .world_mut()
                .spawn((
                    TreeNode(source),
                    TreeNodeExpanded(false),
                    Node::default(),
                    ChildOf(tree),
                ))
                .id();
            app.world_mut().entity_mut(content).insert(ChildOf(row));
            row
        })
        .collect();
    app.update();
    (app, rows)
}

fn press(app: &mut App, key: KeyCode) {
    // No `InputPlugin` here, so nothing ages the press out between
    // frames: reset first, or the second press is never a new one.
    {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.reset_all();
        input.press(key);
    }
    app.update();
}

/// The outliner walks on the arrow keys, which it reads itself. A chord
/// being recorded must not walk it.
#[test]
fn recording_takes_the_trees_arrow_keys() {
    let (mut app, rows) = tree_app();
    app.world_mut().resource_mut::<TreeFocused>().0 = Some(rows[0]);

    app.world_mut().resource_mut::<KeymapCapture>().recording = true;
    press(&mut app, KeyCode::ArrowDown);
    assert_eq!(
        app.world().resource::<TreeFocused>().0,
        Some(rows[0]),
        "the press was naming a chord, not walking the tree",
    );

    app.world_mut().resource_mut::<KeymapCapture>().recording = false;
    press(&mut app, KeyCode::ArrowDown);
    assert_eq!(
        app.world().resource::<TreeFocused>().0,
        Some(rows[1]),
        "and the same press still walks it when nothing is recording",
    );
}

/// The draw-brush chords hang off marker actions rather than off the
/// operator, so the dispatcher's gate never sees them.
#[test]
fn recording_takes_the_draw_brush_chord() {
    let mut app = util::editor_test_app();

    let running = |app: &mut App| -> bool {
        app.world_mut()
            .query_filtered::<(), With<jackdaw_api_internal::lifecycle::ActiveModalOperator>>()
            .iter(app.world())
            .next()
            .is_some()
    };

    let context = app.world_mut().spawn_empty().id();
    let cut = move |app: &mut App| {
        app.world_mut()
            .trigger(bevy_enhanced_input::prelude::Start::<
                jackdaw::draw_brush::StartDrawBrushCutAction,
            > {
                context,
                action: context,
                value: true,
                state: bevy_enhanced_input::prelude::TriggerState::Fired,
            });
        app.update();
        app.update();
    };

    app.world_mut().resource_mut::<KeymapCapture>().recording = true;
    cut(&mut app);
    assert!(
        !running(&mut app),
        "the press was naming a chord, not cutting a brush",
    );

    app.world_mut().resource_mut::<KeymapCapture>().recording = false;
    cut(&mut app);
    assert!(
        running(&mut app),
        "and the same chord still starts the cut when nothing is recording",
    );
}
