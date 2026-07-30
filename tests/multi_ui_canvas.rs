use std::time::Duration;

use bevy::{
    camera::NormalizedRenderTarget,
    picking::{
        backend::HitData,
        events::{Click, Pointer},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    window::WindowRef,
};
use jackdaw::{
    add_entity_picker::collect_add_menu_items,
    selection::Selection,
    ui_canvas::{UI_CANVAS_WINDOW_ID, UiCanvasPanelHost},
    ui_projection::{ProjectedFrom, UiProjection},
};
use jackdaw_api_internal::lifecycle::enable_extension;
use jackdaw_panels::WindowRegistry;
use jackdaw_scene_types::SceneNodeId;
use jackdaw_ui::{UiButton, UiCanvas};
use jackdaw_widgets::menu_bar::MenuAction;

mod util;

#[test]
fn ui_canvas_is_a_multi_instance_window_with_isolated_lifecycle() {
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
    let canvas = app
        .world_mut()
        .spawn((Name::new("Main UI"), UiCanvas::default(), SceneNodeId(900)))
        .id();
    let host_a = app.world_mut().spawn(Node::default()).id();
    let host_b = app.world_mut().spawn(Node::default()).id();
    build(&mut ChildSpawner::new(app.world_mut(), host_a));
    build(&mut ChildSpawner::new(app.world_mut(), host_b));
    app.update();

    let invalid_visibility_parents = {
        let world = app.world_mut();
        let mut query = world.query_filtered::<(Entity, &ChildOf), With<InheritedVisibility>>();
        query
            .iter(world)
            .filter(|(_, parent)| world.get::<InheritedVisibility>(parent.parent()).is_none())
            .map(|(entity, parent)| (entity, parent.parent()))
            .collect::<Vec<_>>()
    };
    assert!(
        invalid_visibility_parents.is_empty(),
        "visibility hierarchy violations: {invalid_visibility_parents:?}"
    );

    let panel_a = *app
        .world()
        .get::<UiCanvasPanelHost>(host_a)
        .expect("first panel host");
    let panel_b = *app
        .world()
        .get::<UiCanvasPanelHost>(host_b)
        .expect("second panel host");
    assert_eq!(panel_a.canvas, Some(canvas));
    assert_eq!(panel_b.canvas, Some(canvas));
    assert_ne!(panel_a.camera, panel_b.camera);
    assert_ne!(panel_a.projection, panel_b.projection);

    let root_a = UiProjection::root(app.world(), panel_a.projection.unwrap()).unwrap();
    let root_b = UiProjection::root(app.world(), panel_b.projection.unwrap()).unwrap();
    for root in [root_a, root_b] {
        assert!(
            app.world().entity(root).contains::<InheritedVisibility>(),
            "projected UI roots participate in Bevy visibility propagation"
        );
    }
    app.world_mut().entity_mut(host_a).despawn();
    app.update();

    assert!(app.world().get_entity(panel_a.camera).is_err());
    assert!(app.world().get_entity(root_a).is_err());
    assert!(app.world().get_entity(panel_b.camera).is_ok());
    assert!(app.world().get_entity(root_b).is_ok());
}

#[test]
fn fresh_ui_canvas_exposes_widget_creation_from_every_add_surface() {
    let mut app = util::editor_test_app();
    let _ = enable_extension(app.world_mut(), "jackdaw.ui_editor");
    app.update();

    let items = collect_add_menu_items(app.world_mut());
    assert!(
        items
            .iter()
            .any(|item| item.label == "Canvas" && item.action == "widget:layout.canvas"),
        "the shared Add source must expose a UI Canvas"
    );
    assert!(
        items
            .iter()
            .any(|item| item.label == "Button" && item.action == "widget:feathers.button"),
        "the shared Add source must expose Feathers widgets"
    );

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

    let visible_labels = {
        let world = app.world_mut();
        let mut query = world.query::<(Entity, &Text)>();
        query
            .iter(world)
            .filter(|(entity, _)| is_descendant_of(world, *entity, host))
            .map(|(_, text)| text.as_str().to_string())
            .collect::<Vec<_>>()
    };
    assert!(
        visible_labels.iter().any(|label| label == "Canvas"),
        "a fresh Canvas window must expose its Canvas palette item: {visible_labels:?}"
    );
    assert!(
        visible_labels.iter().any(|label| label == "Button"),
        "a fresh Canvas window must expose its Button palette item: {visible_labels:?}"
    );

    app.world_mut().trigger(MenuAction {
        action: "widget:feathers.button".to_string(),
    });
    app.update();

    let canvas = app
        .world_mut()
        .query_filtered::<Entity, (With<UiCanvas>, Without<ProjectedFrom>)>()
        .single(app.world())
        .expect("one authored canvas");
    let button = app
        .world_mut()
        .query_filtered::<Entity, (With<UiButton>, Without<ProjectedFrom>)>()
        .single(app.world())
        .expect("one authored button");
    let panel = *app
        .world()
        .get::<UiCanvasPanelHost>(host)
        .expect("Canvas panel host");
    let button_node = *app
        .world()
        .get::<SceneNodeId>(button)
        .expect("authored button node id");
    let projected_button =
        UiProjection::projected_entity(app.world(), panel.projection.unwrap(), button_node)
            .expect("projected button");
    assert_ne!(
        app.world().get::<Visibility>(projected_button),
        Some(&Visibility::Hidden),
        "the Canvas projection must remain visible"
    );

    let pointer_window = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: NormalizedRenderTarget::Window(
                WindowRef::Entity(pointer_window).normalize(None).unwrap(),
            ),
            position: Vec2::ZERO,
        },
        Click {
            button: PointerButton::Primary,
            hit: HitData {
                camera: panel.camera,
                depth: 0.0,
                position: None,
                normal: None,
                extra: None,
            },
            duration: Duration::ZERO,
            count: 1,
        },
        projected_button,
    ));
    app.update();
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(button),
        "clicking projected UI in Mode: Select must select the authored widget"
    );
    assert_eq!(
        app.world().get::<Visibility>(canvas),
        Some(&Visibility::Inherited),
        "editor suppression must not change the authored scene visibility"
    );
    for authored in [canvas, button] {
        assert_eq!(
            app.world().get::<InheritedVisibility>(authored),
            Some(&InheritedVisibility::HIDDEN),
            "authored UI source {authored} must not leak into the editor's default UI camera"
        );
    }
}

fn is_descendant_of(world: &World, entity: Entity, ancestor: Entity) -> bool {
    let mut current = Some(entity);
    while let Some(entity) = current {
        if entity == ancestor {
            return true;
        }
        current = world.get::<ChildOf>(entity).map(ChildOf::parent);
    }
    false
}

/// A widget's implementation-owned parts and every per-view projection are
/// derived state. Neither may reach the Outliner, and the authored entity must
/// stay inert reflected data: the Components tab and the saved document both
/// show exactly what the user authored.
#[test]
fn authored_ui_stays_pure_data_and_derived_entities_stay_out_of_the_outliner() {
    let mut app = util::editor_test_app();
    let _ = enable_extension(app.world_mut(), "jackdaw.ui_editor");
    app.update();

    let canvas_build = app
        .world()
        .resource::<WindowRegistry>()
        .get(UI_CANVAS_WINDOW_ID)
        .expect("UI Canvas window registered")
        .build
        .clone();

    app.world_mut().spawn((
        jackdaw::hierarchy::HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    let canvas_host = app.world_mut().spawn(Node::default()).id();
    canvas_build(&mut ChildSpawner::new(app.world_mut(), canvas_host));
    app.update();

    app.world_mut().trigger(MenuAction {
        action: "widget:feathers.button".to_string(),
    });
    for _ in 0..4 {
        app.update();
    }

    let world = app.world_mut();
    let rowed: Vec<Entity> = world
        .query::<&jackdaw_widgets::tree_view::TreeNode>()
        .iter(world)
        .map(|node| node.0)
        .collect();
    for source in rowed {
        assert!(
            world.get::<jackdaw_ui::UiGeneratedPart>(source).is_none(),
            "generated widget part {source} must not have an Outliner row"
        );
        assert!(
            world.get::<ProjectedFrom>(source).is_none(),
            "projected entity {source} must not have an Outliner row"
        );
    }

    let button = world
        .query_filtered::<Entity, (With<UiButton>, Without<ProjectedFrom>)>()
        .single(world)
        .expect("one authored button");
    for implementation in [
        "bevy_feathers::controls::button::FeathersButton",
        "bevy_feathers::theme::ThemeBackgroundColor",
        "bevy_ui_widgets::button::Button",
    ] {
        let present = world
            .entity(button)
            .archetype()
            .components()
            .iter()
            .any(|id| world.components().get_name(*id).as_deref() == Some(implementation));
        assert!(
            !present,
            "authored button must not carry the implementation component {implementation}"
        );
    }
    assert!(
        world
            .get::<Children>(button)
            .is_none_or(bevy::prelude::RelationshipTarget::is_empty),
        "authored button must have no generated children"
    );

    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast
        .ast_for(button)
        .expect("authored button is in the document");
    let paths = ast.component_type_paths(node);
    assert!(
        paths.iter().any(|path| path == "jackdaw_ui::UiButton"),
        "the authored widget component must persist: {paths:?}"
    );
    for computed in [
        "bevy_ui::ui_node::ComputedNode",
        "bevy_ui::ui_node::ComputedUiTargetCamera",
        "bevy_ui::ui_transform::UiGlobalTransform",
        "bevy_ui::stack::ComputedStackIndex",
    ] {
        assert!(
            !paths.iter().any(|path| path == computed),
            "computed UI state {computed} must not enter the document: {paths:?}"
        );
    }
}
