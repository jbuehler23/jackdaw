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
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::KeymapCapture;
use jackdaw_widgets::tree_view::{
    TreeFocused, TreeNode, TreeNodeExpanded, TreeRoot, TreeRowContent,
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

    let tree = app.world_mut().spawn((TreeRoot, Node::default())).id();
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

/// Escape is a chord like any other, so a user must be able to bind it.
/// It only works if the dialog it is being recorded in stops treating it
/// as the way out: the press names the chord, and the dialog is still
/// standing afterwards to show what it named.
#[test]
fn escape_pressed_while_recording_binds_escape_and_leaves_the_dialog_up() {
    let mut app = util::editor_test_app();
    open_keybind_dialog(&mut app);
    let operator = "entity.delete";
    let button = rebind_button(&mut app, operator);
    app.world_mut()
        .trigger(jackdaw_feathers::button::ButtonClickEvent { entity: button });
    app.update();
    assert!(
        app.world().resource::<KeymapCapture>().recording,
        "pressing Rebind starts a recording",
    );

    // Escape is already claimed by other commands, so the first press
    // raises the confirmation the dialog asks for and the second commits
    // it. Neither may reach the dialog's own Escape handler.
    press_escape(&mut app);
    assert!(
        dialog_is_open(&mut app),
        "the Escape that named a chord did not also close the dialog",
    );
    press_escape(&mut app);
    assert!(
        dialog_is_open(&mut app),
        "nor did the Escape that confirmed it",
    );

    let chords = app
        .world()
        .resource::<jackdaw::keybind_settings::PendingKeymapChanges>()
        .chords_of(operator);
    assert_eq!(
        chords,
        vec!["Esc".to_string()],
        "and it landed on the row as the row's binding",
    );
    assert!(
        !app.world().resource::<KeymapCapture>().recording,
        "the recording ended when the chord was taken",
    );

    // The other half of the same gate: with nothing recording, Escape is
    // still the way out of the dialog.
    press_escape(&mut app);
    app.update();
    assert!(
        !dialog_is_open(&mut app),
        "Escape still closes a dialog that is not recording",
    );
}

/// The numeric transform entry arms on a bare X / Y / Z, which it reads
/// itself. A chord being recorded must not arm it, or the press that names
/// `X` also starts a transform behind the dialog.
#[test]
fn recording_takes_the_numeric_transform_axis_keys() {
    use jackdaw::numeric_transform::NumericTransformState;

    let mut app = util::editor_test_app();
    let entity = app
        .world_mut()
        .spawn((Name::new("Thing"), Transform::default()))
        .id();
    jackdaw::selection::select_only(app.world_mut(), entity);
    app.update();

    // The plugin schedules the reader inside `EditorInteractionSystems`,
    // which a headless app never reaches, so run it directly.
    let arm = |app: &mut App| {
        {
            let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            input.reset_all();
            input.press(KeyCode::KeyX);
        }
        jackdaw::numeric_transform::run_numeric_transform_input(app.world_mut());
    };

    app.world_mut().resource_mut::<KeymapCapture>().recording = true;
    arm(&mut app);
    assert!(
        app.world()
            .resource::<NumericTransformState>()
            .axis
            .is_none(),
        "the press was naming a chord, not arming an axis",
    );

    app.world_mut().resource_mut::<KeymapCapture>().recording = false;
    arm(&mut app);
    assert!(
        app.world()
            .resource::<NumericTransformState>()
            .axis
            .is_some(),
        "and the same press still arms it when nothing is recording",
    );
}

/// The node graph's add-node popover closes on Escape, which it reads
/// itself. A chord being recorded must not close it.
#[test]
fn recording_takes_the_add_node_popovers_escape() {
    use jackdaw_node_graph::add_node_popover::{
        AddNodeBackdrop, AddNodePopover, handle_popover_escape,
    };

    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<KeymapCapture>()
        .add_systems(Update, handle_popover_escape);
    let graph = app.world_mut().spawn_empty().id();
    let backdrop = app.world_mut().spawn_empty().id();
    let popover = app
        .world_mut()
        .spawn(AddNodePopover {
            graph,
            spawn_position: Vec2::ZERO,
            backdrop,
        })
        .id();
    app.world_mut()
        .entity_mut(backdrop)
        .insert(AddNodeBackdrop { popover });
    app.update();

    app.world_mut().resource_mut::<KeymapCapture>().recording = true;
    press(&mut app, KeyCode::Escape);
    assert!(
        app.world().get_entity(popover).is_ok(),
        "the press was naming a chord, not dismissing the popover",
    );

    app.world_mut().resource_mut::<KeymapCapture>().recording = false;
    press(&mut app, KeyCode::Escape);
    assert!(
        app.world().get_entity(popover).is_err(),
        "and the same press still dismisses it when nothing is recording",
    );
}

/// Escape as the editor sees it: a keyboard message the input pass turns
/// into `just_pressed` on the frame it is read. A press written straight
/// into `ButtonInput` would be cleared by that pass before any system saw
/// it, which is why an app with the input plugin goes through a message.
fn press_escape(app: &mut App) {
    use bevy::input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    };
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    // Down and up, because a second press of a key still held is not a
    // new press: this test presses Escape several times over.
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: Key::Escape,
            state,
            text: None,
            repeat: false,
            window,
        });
        app.update();
    }
}

/// Open the keybind settings dialog the way the menu row does.
///
/// The dialog's systems are gated on `AppState::Editor`, which a headless
/// app starts outside of, so the state is entered first: a dialog nothing
/// populates has no row to record on.
fn open_keybind_dialog(app: &mut App) {
    // The editor's own systems expect an open project; without one the
    // panels that read it fail their parameter validation on the first
    // frame in the state.
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: std::env::temp_dir().join("jackdaw_capture_gate_project"),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }
    let result = app
        .world_mut()
        .operator("app.open_keybinds")
        .call()
        .expect("the keybind dialog opens through an operator");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..4 {
        app.update();
    }
}

fn dialog_is_open(app: &mut App) -> bool {
    app.world_mut()
        .query_filtered::<(), With<jackdaw_feathers::dialog::EditorDialog>>()
        .iter(app.world())
        .next()
        .is_some()
}

/// The Rebind button on `operator`'s row.
fn rebind_button(app: &mut App, operator: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &jackdaw::keybind_settings::KeybindRebindButton)>()
        .iter(app.world())
        .find(|(_, button)| button.0 == operator)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the dialog shows a Rebind button for {operator}"))
}
