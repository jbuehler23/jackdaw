mod util;

use bevy::prelude::*;
use jackdaw::{
    commands::{CommandHistory, HierarchyLocation, MoveEntity},
    selection::Selection,
    ui_authoring::UiAuthoring,
};
use jackdaw_api::{WidgetDefinition, WidgetInstantiateContext, WidgetRegistry};
use jackdaw_api_internal::lifecycle::{disable_extension, enable_extension};
use jackdaw_bsn::SceneBsnAst;
use jackdaw_commands::EditorCommand;
use jackdaw_ui::{UiButton, UiCanvas};

#[test]
fn built_in_widgets_create_authored_undoable_hierarchy() {
    let mut app = util::editor_test_app();
    ensure_ui_editor(&mut app);

    let canvas = UiAuthoring::instantiate(
        app.world_mut(),
        "layout.canvas",
        WidgetInstantiateContext::default(),
    )
    .expect("Canvas definition should instantiate");
    let button = UiAuthoring::instantiate(
        app.world_mut(),
        "feathers.button",
        WidgetInstantiateContext {
            parent: Some(canvas),
        },
    )
    .expect("Button definition should instantiate");

    assert!(app.world().get::<UiCanvas>(canvas).is_some());
    assert!(app.world().get::<UiButton>(button).is_some());
    assert_eq!(
        app.world().get::<ChildOf>(button).map(ChildOf::parent),
        Some(canvas)
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(button),
        "new widget should become the shared editor selection"
    );

    let ast = app.world().resource::<SceneBsnAst>();
    let canvas_ast = ast.ast_for(canvas).expect("canvas should be authored");
    let button_ast = ast.ast_for(button).expect("button should be authored");
    assert_eq!(ast.get_children_ast(canvas_ast), vec![button_ast]);
    assert_eq!(app.world().resource::<CommandHistory>().undo_stack.len(), 2);

    let mut history = std::mem::take(
        app.world_mut()
            .resource_mut::<CommandHistory>()
            .into_inner(),
    );
    history.undo(app.world_mut());
    *app.world_mut().resource_mut::<CommandHistory>() = history;

    assert!(app.world().get_entity(button).is_err());
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(button)
            .is_none()
    );
}

#[test]
fn ui_sibling_order_is_undoable_and_matches_the_document() {
    let mut app = util::editor_test_app();
    ensure_ui_editor(&mut app);
    let canvas = UiAuthoring::instantiate(
        app.world_mut(),
        "layout.canvas",
        WidgetInstantiateContext::default(),
    )
    .unwrap();
    let first = UiAuthoring::instantiate(
        app.world_mut(),
        "feathers.button",
        WidgetInstantiateContext {
            parent: Some(canvas),
        },
    )
    .unwrap();
    let second = UiAuthoring::instantiate(
        app.world_mut(),
        "feathers.button",
        WidgetInstantiateContext {
            parent: Some(canvas),
        },
    )
    .unwrap();

    let mut command = MoveEntity::new(
        app.world(),
        first,
        HierarchyLocation {
            parent: Some(canvas),
            index: 1,
            slot: Some("content".into()),
        },
    );
    command.execute(app.world_mut());

    assert_eq!(
        app.world()
            .get::<Children>(canvas)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        vec![second, first]
    );
    let ast = app.world().resource::<SceneBsnAst>();
    let canvas_ast = ast.ast_for(canvas).unwrap();
    assert_eq!(
        ast.get_children_ast(canvas_ast),
        vec![ast.ast_for(second).unwrap(), ast.ast_for(first).unwrap()]
    );

    command.undo(app.world_mut());
    assert_eq!(
        app.world()
            .get::<Children>(canvas)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        vec![first, second]
    );
}

#[test]
fn widget_definitions_follow_extension_lifecycle() {
    let mut app = util::editor_test_app();
    ensure_ui_editor(&mut app);
    assert!(
        app.world()
            .resource::<WidgetRegistry>()
            .get("feathers.slider")
            .is_some()
    );

    assert!(disable_extension(app.world_mut(), "jackdaw.ui_editor"));
    app.update();

    assert!(
        app.world()
            .resource::<WidgetRegistry>()
            .get("feathers.slider")
            .is_none()
    );
}

#[test]
fn extension_widget_callbacks_inherit_authoring_undo_and_selection() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<WidgetRegistry>()
        .register(WidgetDefinition::new(
            "sample.badge",
            "Badge",
            "Custom",
            |world, context| {
                let mut badge = world.spawn((Name::new("Badge"), Node::default()));
                if let Some(parent) = context.parent {
                    badge.insert(ChildOf(parent));
                }
                Ok(badge.id())
            },
        ));

    let badge = UiAuthoring::instantiate(
        app.world_mut(),
        "sample.badge",
        WidgetInstantiateContext::default(),
    )
    .unwrap();

    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(badge)
            .is_some(),
        "the editor facade authors extension-created entities automatically"
    );
    assert_eq!(app.world().resource::<Selection>().primary(), Some(badge));
}

fn ensure_ui_editor(app: &mut App) {
    let _ = enable_extension(app.world_mut(), "jackdaw.ui_editor");
}
