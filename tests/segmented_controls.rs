//! The Play/Select and Scene/Live bars and the workspace tab strip.
//!
//! All three are radio groups: the bar carries `RadioGroup`, each segment
//! carries `RadioButton`, the current choice carries `Checked`, and a
//! click reaches the group as a `ValueChange<Entity>` naming the segment.

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::picking::{
    backend::HitData,
    events::{Click, Pointer},
    pointer::{Location, PointerButton, PointerId},
};
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{RadioButton, RadioGroup};
use bevy::window::{PrimaryWindow, WindowRef};

use jackdaw::game_panel::{GameModeSegment, GamePanelMode, game_panel_content};
use jackdaw::pie_mirror::{PieViewMode, PieViewSegment};

mod util;

/// Click `entity` the way a user does: the `Pointer<Click>` the radio
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
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        Click {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: core::time::Duration::ZERO,
            count: 1,
        },
        entity,
    ));
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
    assert!(
        app.world().get::<Interaction>(segment).is_none(),
        "and not a hand-rolled interaction control",
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

fn game_app() -> App {
    let mut app = util::editor_test_app();
    app.world_mut().spawn(game_panel_content());
    app.update();
    app
}

/// The Play/Select bar is a radio group, and the mode the panel is in is
/// the checked segment.
#[test]
fn the_game_mode_bar_is_a_radio_group() {
    let mut app = game_app();

    let play = segment_of(&mut app, &GameModeSegment::Play);
    let select = segment_of(&mut app, &GameModeSegment::Select);
    assert_is_a_segment(&app, play);
    assert_is_a_segment(&app, select);

    app.update();
    let checked = if *app.world().resource::<GamePanelMode>() == GamePanelMode::Play {
        play
    } else {
        select
    };
    assert!(
        app.world().get::<Checked>(checked).is_some(),
        "the mode the panel is in is the checked segment",
    );
}

/// Clicking a segment moves the panel's mode, the effect the bar existed
/// for before it was a radio group.
#[test]
fn clicking_a_game_mode_segment_moves_the_mode() {
    let mut app = game_app();
    let start = *app.world().resource::<GamePanelMode>();
    let other = match start {
        GamePanelMode::Play => GameModeSegment::Select,
        GamePanelMode::Select => GameModeSegment::Play,
    };
    let wanted = match other {
        GameModeSegment::Play => GamePanelMode::Play,
        GameModeSegment::Select => GamePanelMode::Select,
    };

    let segment = segment_of(&mut app, &other);
    click(&mut app, segment);
    app.update();

    assert_eq!(
        *app.world().resource::<GamePanelMode>(),
        wanted,
        "the click reached the mode through the group",
    );
    assert!(
        app.world().get::<Checked>(segment).is_some(),
        "and the segment it named is now the checked one",
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
