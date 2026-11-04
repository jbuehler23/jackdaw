//! The Scene/Live bar and the workspace tab strip.
//!
//! Both are radio groups: the bar carries `RadioGroup`, each segment
//! carries `RadioButton`, the current choice carries `Checked`, and a
//! click reaches the group as a `ValueChange<Entity>` naming the segment.

use crate::util;

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::picking::{
    backend::HitData,
    events::{Pointer, PointerClick},
    pointer::{Location, PointerButton, PointerId},
};
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{RadioButton, RadioGroup};
use bevy::window::{PrimaryWindow, WindowRef};

use jackdaw::pie_mirror::{PieViewMode, PieViewSegment};

/// Click `entity` the way a user does: the `PointerClick` the radio
/// widget is watching for.
fn click(app: &mut App, entity: Entity) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(PointerClick {
        entity,
        pointer: Pointer::new(
            PointerId::Mouse,
            Location {
                target,
                position: Vec2::ZERO,
            },
        ),
        button: PointerButton::Primary,
        hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
        duration: core::time::Duration::ZERO,
        count: 1,
    });
    app.update();
}

fn segment_of<T: Component + PartialEq>(app: &mut App, wanted: &T) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &T)>()
        .iter(app.world())
        .filter(|(_, segment)| *segment == wanted)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one segment per choice");
    found[0]
}

fn assert_is_a_segment(app: &App, segment: Entity) {
    assert!(
        app.world().get::<RadioButton>(segment).is_some(),
        "a segment is a radio button",
    );
    let bar = app
        .world()
        .get::<ChildOf>(segment)
        .expect("a segment sits in a bar")
        .parent();
    assert!(
        app.world().get::<RadioGroup>(bar).is_some(),
        "the bar the segments share is the radio group",
    );
}

/// The Scene/Live toggle is a radio group too, with Scene checked while
/// the editor is showing the authored scene.
#[test]
fn the_scene_live_toggle_is_a_radio_group() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::hierarchy_content(default()));
    app.update();
    // The appearance pass is scheduled behind `AppState::Editor`, which a
    // headless test never enters.
    app.world_mut()
        .run_system_cached(jackdaw::layout::update_pie_view_toggle_appearance)
        .expect("the toggle's appearance pass runs");
    app.update();

    let scene = segment_of(&mut app, &PieViewSegment::Scene);
    let live = segment_of(&mut app, &PieViewSegment::Live);
    assert_is_a_segment(&app, scene);
    assert_is_a_segment(&app, live);

    assert_eq!(*app.world().resource::<PieViewMode>(), PieViewMode::Scene);
    assert!(
        app.world().get::<Checked>(scene).is_some(),
        "the view the editor is in is the checked segment",
    );
    assert!(app.world().get::<Checked>(live).is_none());
}

/// The workspace tab strip is a radio group: the strip is the group,
/// each tab a radio button, and the workspace in view carries `Checked`.
#[test]
fn the_workspace_tab_strip_is_a_radio_group() {
    use jackdaw_panels::workspace::{WorkspaceRegistry, WorkspaceTab, WorkspaceTabStrip};

    let mut app = util::editor_test_app();
    let strip = app
        .world_mut()
        .spawn((WorkspaceTabStrip, Node::default()))
        .id();
    app.world_mut()
        .run_system_cached(jackdaw_panels::workspace_tabs::populate_workspace_tabs)
        .expect("the strip populates");
    app.update();

    assert!(
        app.world().get::<RadioGroup>(strip).is_some(),
        "the strip is the radio group",
    );

    let active = app
        .world()
        .resource::<WorkspaceRegistry>()
        .active
        .clone()
        .expect("the editor opens in a workspace");
    let tabs: Vec<(Entity, String)> = app
        .world_mut()
        .query::<(Entity, &WorkspaceTab)>()
        .iter(app.world())
        .map(|(entity, tab)| (entity, tab.workspace_id.clone()))
        .collect();
    assert!(tabs.len() > 1, "the editor ships more than one workspace");

    for (entity, id) in &tabs {
        assert_is_a_segment(&app, *entity);
        assert_eq!(
            app.world().get::<Checked>(*entity).is_some(),
            *id == active,
            "the workspace in view is the checked tab",
        );
    }

    let (other, other_id) = tabs
        .iter()
        .find(|(_, id)| *id != active)
        .cloned()
        .expect("a workspace that is not the one in view");
    click(&mut app, other);
    app.update();

    assert_eq!(
        app.world().resource::<WorkspaceRegistry>().active.as_ref(),
        Some(&other_id),
        "choosing a tab swaps the workspace, as its click always did",
    );
}
