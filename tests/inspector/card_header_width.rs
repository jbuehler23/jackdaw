//! A component card's header keeps room for the card's name in a narrow panel.

use crate::util;

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use jackdaw::selection::Selection;
use jackdaw_widgets::collapsible::CollapsibleHeader;

/// The inspector mounted in a dock leaf `width` wide, with an entity carrying
/// a `Transform` selected.
fn panel(width: f32) -> App {
    let mut app = util::editor_test_app();
    let leaf = app
        .world_mut()
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Px(width),
            height: Val::Px(900.0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            ..default()
        })
        .id();
    let content = app
        .world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()))
        .id();
    app.world_mut().entity_mut(content).insert(ChildOf(leaf));
    let entity = app
        .world_mut()
        .spawn((Name::new("moved"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    for _ in 0..4 {
        app.update();
    }
    app
}

/// Each card header with its children: the title row first, then its buttons.
fn headers(app: &mut App) -> Vec<(Entity, Vec<Entity>)> {
    let mut query = app
        .world_mut()
        .query_filtered::<(Entity, &Children), With<CollapsibleHeader>>();
    query
        .iter(app.world())
        .map(|(header, children)| (header, children.iter().collect()))
        .collect()
}

/// Left edge, right edge and vertical centre.
fn bounds(app: &App, entity: Entity) -> (f32, f32, f32) {
    let computed = app.world().get::<ComputedNode>(entity).unwrap();
    let transform = app.world().get::<UiGlobalTransform>(entity).unwrap();
    let half = computed.size().x / 2.0;
    let x = transform.translation.x;
    (x - half, x + half, transform.translation.y)
}

#[test]
fn a_narrow_panel_wraps_the_header_buttons_rather_than_hide_the_name() {
    let mut app = panel(110.0);
    let headers = headers(&mut app);
    assert!(!headers.is_empty(), "the selection builds a card");
    for (header, children) in headers {
        let (header_left, header_right, _) = bounds(&app, header);
        let (title, buttons) = children.split_first().unwrap();
        let (title_left, title_right, title_y) = bounds(&app, *title);
        for &button in buttons {
            let (_, _, button_y) = bounds(&app, button);
            assert!(
                button_y > title_y,
                "a header button shares the row with a title {} px wide",
                title_right - title_left,
            );
        }
        assert!(
            title_right - title_left >= 0.75 * (header_right - header_left),
            "the title row is {} px of a {} px header",
            title_right - title_left,
            header_right - header_left,
        );
    }
}

#[test]
fn a_panel_with_room_keeps_the_header_on_one_row() {
    let mut app = panel(231.0);
    for (_, children) in headers(&mut app) {
        let (title, buttons) = children.split_first().unwrap();
        let (_, title_right, title_y) = bounds(&app, *title);
        for &button in buttons {
            let (button_left, _, button_y) = bounds(&app, button);
            assert!(
                (button_y - title_y).abs() < 0.5 && button_left >= title_right,
                "a header button left the title's row",
            );
        }
    }
}
