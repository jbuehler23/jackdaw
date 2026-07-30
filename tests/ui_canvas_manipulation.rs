use bevy::{
    camera::NormalizedRenderTarget,
    picking::{
        backend::HitData,
        events::{Drag, DragEnd, DragStart, Pointer},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    window::WindowRef,
};
use jackdaw::{
    commands::CommandHistory,
    selection::Selection,
    ui_canvas::{UI_CANVAS_WINDOW_ID, UiCanvasPanelHost},
    ui_projection::ProjectedFrom,
};
use jackdaw_api_internal::lifecycle::enable_extension;
use jackdaw_panels::WindowRegistry;
use jackdaw_ui::UiButton;
use jackdaw_widgets::menu_bar::MenuAction;

mod util;

fn pointer_event<E: std::fmt::Debug + Clone + Reflect>(
    world: &mut World,
    target: Entity,
    _camera: Entity,
    event: E,
) {
    // `PointerTraversal` climbs to the pointer's window entity and stops only
    // if that entity carries `Window`; a bare entity makes propagation loop.
    let window = world.spawn(Window::default()).id();
    world.trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: NormalizedRenderTarget::Window(
                WindowRef::Entity(window).normalize(None).unwrap(),
            ),
            position: Vec2::ZERO,
        },
        event,
        target,
    ));
}

/// Dragging the selection overlay moves the authored node, mirrors into the
/// projection without a rebuild, and lands in the history as one entry that
/// undo restores exactly.
#[test]
fn dragging_the_canvas_overlay_edits_authored_layout_as_one_undo_step() {
    let mut app = util::editor_test_app();
    let _ = enable_extension(app.world_mut(), "jackdaw.ui_editor");
    app.update();

    let build = app
        .world()
        .resource::<WindowRegistry>()
        .get(UI_CANVAS_WINDOW_ID)
        .expect("UI Canvas window registered")
        .build
        .clone();
    let host = app.world_mut().spawn(Node::default()).id();
    build(&mut ChildSpawner::new(app.world_mut(), host));
    app.update();

    app.world_mut().trigger(MenuAction {
        action: "widget:feathers.button".to_string(),
    });
    for _ in 0..4 {
        app.update();
    }

    let button = app
        .world_mut()
        .query_filtered::<Entity, (With<UiButton>, Without<ProjectedFrom>)>()
        .single(app.world())
        .expect("one authored button");
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(button),
        "a created widget is selected so it can be edited immediately"
    );

    let panel = *app
        .world()
        .get::<UiCanvasPanelHost>(host)
        .expect("Canvas panel host");
    let overlay = app
        .world_mut()
        .query_filtered::<Entity, With<Node>>()
        .iter(app.world())
        .find(|entity| {
            app.world().get::<ChildOf>(*entity).map(ChildOf::parent) == Some(panel.stage)
                && app.world().get::<ZIndex>(*entity) == Some(&ZIndex(50))
        })
        .expect("a selection overlay covers the selected widget");

    let before = app.world().get::<Node>(button).cloned().expect("node");
    let undo_before = app.world().resource::<CommandHistory>().undo_stack.len();

    pointer_event(
        app.world_mut(),
        overlay,
        panel.camera,
        DragStart {
            button: PointerButton::Primary,
            hit: HitData {
                camera: panel.camera,
                depth: 0.0,
                position: None,
                normal: None,
                extra: None,
            },
        },
    );
    app.update();
    pointer_event(
        app.world_mut(),
        overlay,
        panel.camera,
        Drag {
            button: PointerButton::Primary,
            distance: Vec2::new(40.0, 25.0),
            delta: Vec2::new(40.0, 25.0),
        },
    );
    app.update();
    pointer_event(
        app.world_mut(),
        overlay,
        panel.camera,
        DragEnd {
            button: PointerButton::Primary,
            distance: Vec2::new(40.0, 25.0),
        },
    );
    app.update();

    let after = app.world().get::<Node>(button).cloned().expect("node");
    assert_ne!(after, before, "the drag must move the authored node");
    assert_eq!(
        after.position_type,
        PositionType::Absolute,
        "a free move promotes a flex child to absolute placement"
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        undo_before + 1,
        "one drag is one history entry"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    app.update();
    assert_eq!(
        app.world().get::<Node>(button).cloned(),
        Some(before),
        "undo restores the exact node the drag started from"
    );
}
