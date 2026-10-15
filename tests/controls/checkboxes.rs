//! Feathers checkboxes as the editor draws them: a disabled one refuses a
//! click and still reads as a checkbox, dimmed.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::{ThemedText, UiTheme};
use bevy::feathers::tokens;
use bevy::prelude::*;
use bevy::ui::{Checked, InteractionDisabled, UiGlobalTransform};
use bevy::ui_widgets::ValueChange;
use bevy::window::{PrimaryWindow, WindowResolution};
use jackdaw::test_input::SyntheticInput;

#[derive(Resource, Default)]
struct Changed(Vec<Entity>);

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn play(app: &mut App) {
    for _ in 0..200 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    settle(app);
}

fn app() -> App {
    let mut app = util::editor_test_app();
    let mut windows = app
        .world_mut()
        .query_filtered::<&mut Window, With<PrimaryWindow>>();
    windows
        .single_mut(app.world_mut())
        .expect("headless apps still have a primary window")
        .resolution = WindowResolution::new(1600, 1000);
    app.init_resource::<Changed>();
    app.add_observer(
        |change: On<ValueChange<bool>>, mut changed: ResMut<Changed>| {
            changed.0.push(change.source);
        },
    );
    app
}

/// A column the checkboxes are laid out in, away from the window's edges.
fn column(app: &mut App) -> Entity {
    app.world_mut()
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(24),
            padding: UiRect::all(px(40)),
            ..default()
        })
        .id()
}

fn checkbox(app: &mut App, column: Entity, caption: &'static str) -> Entity {
    let entity = app
        .world_mut()
        .spawn_scene(bsn! {
            @FeathersCheckbox { @caption: bsn! { Text(caption) ThemedText } }
        })
        .expect("the checkbox spawns")
        .id();
    app.world_mut().entity_mut(entity).insert(ChildOf(column));
    entity
}

/// The box a checkbox draws, which is what a user aims at.
fn outline(app: &App, checkbox: Entity) -> Entity {
    app.world()
        .get::<Children>(checkbox)
        .expect("the checkbox has its box")[0]
}

fn click(app: &mut App, entity: Entity) {
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("placed");
    let computed = app.world().get::<ComputedNode>(entity).expect("laid out");
    let at = transform.translation
        * computed.inverse_scale_factor()
        * app.world().resource::<UiScale>().0;
    jackdaw::boot_ops::run_op_clause(
        app.world_mut(),
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    )
    .expect("the clause dispatches")
    .assert_finished();
    play(app);
}

fn border(app: &App, entity: Entity) -> Color {
    app.world()
        .get::<BorderColor>(entity)
        .expect("a border")
        .top
}

fn caption_color(app: &App, checkbox: Entity) -> Color {
    let caption = app.world().get::<Children>(checkbox).expect("children")[1];
    app.world()
        .get::<TextColor>(caption)
        .expect("a text colour")
        .0
}

#[test]
fn a_disabled_checkbox_refuses_a_click() {
    let mut app = app();
    let column = column(&mut app);
    let enabled = checkbox(&mut app, column, "Enabled");
    let disabled = checkbox(&mut app, column, "Disabled");
    app.world_mut()
        .entity_mut(disabled)
        .insert(InteractionDisabled);
    settle(&mut app);

    let disabled_box = outline(&app, disabled);
    click(&mut app, disabled_box);
    assert!(
        app.world().resource::<Changed>().0.is_empty(),
        "a disabled checkbox reports no change"
    );

    let enabled_box = outline(&app, enabled);
    click(&mut app, enabled_box);
    assert_eq!(
        app.world().resource::<Changed>().0,
        [enabled],
        "a click at the same kind of spot reaches an enabled one"
    );
    assert!(app.world().get::<InteractionDisabled>(disabled).is_some());
}

#[test]
fn a_disabled_checkbox_is_outlined_in_the_disabled_colour() {
    let mut app = app();
    let column = column(&mut app);
    let enabled = checkbox(&mut app, column, "Enabled");
    let unticked = checkbox(&mut app, column, "Unticked");
    let ticked = checkbox(&mut app, column, "Ticked");
    app.world_mut()
        .entity_mut(unticked)
        .insert(InteractionDisabled);
    app.world_mut()
        .entity_mut(ticked)
        .insert((Checked, InteractionDisabled));
    settle(&mut app);
    let unticked_box = outline(&app, unticked);
    click(&mut app, unticked_box);

    let dimmed = app
        .world()
        .resource::<UiTheme>()
        .color(&tokens::CHECKBOX_TEXT_DISABLED);
    assert!(dimmed.alpha() > 0.0);
    for disabled in [unticked, ticked] {
        assert_eq!(border(&app, outline(&app, disabled)), dimmed);
        assert_eq!(caption_color(&app, disabled), dimmed);
    }
    assert_ne!(border(&app, outline(&app, enabled)), dimmed);
    assert_ne!(caption_color(&app, enabled), dimmed);
}
