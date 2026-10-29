//! The `PixelCanvas` card: a resolution preset combo above the reflected
//! fields, writing through `camera.pixel_canvas`.

use crate::util;

use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;
use jackdaw::commands::CommandHistory;
use jackdaw::pixel_camera::{CANVAS_PRESETS, CUSTOM_PRESET, CanvasPresetCombo, PixelCanvasToggle};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, ComboBoxSelectedIndex, ComboBoxTrigger};
use jackdaw_scene_types::PixelCanvas;

fn app_with_canvas(canvas: PixelCanvas) -> (App, Entity) {
    app_with(canvas, true)
}

/// A selected entity carrying `canvas` with the inspector open, a 2D camera
/// when `camera`.
fn app_with(canvas: PixelCanvas, camera: bool) -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    let entity = app
        .world_mut()
        .spawn((Name::new("Game Camera"), canvas, ChildOf(root)))
        .id();
    if camera {
        app.world_mut().entity_mut(entity).insert(Camera2d);
    }
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    for _ in 0..4 {
        app.update();
    }
    (app, entity)
}

fn combo(app: &mut App) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<CanvasPresetCombo>>()
        .iter(app.world())
        .collect();
    assert_eq!(found.len(), 1, "one preset combo");
    found[0]
}

/// The option the combo's trigger draws, by its index.
fn shown(app: &mut App) -> usize {
    let combo = combo(app);
    let trigger = app
        .world_mut()
        .query::<(Entity, &ComboBoxTrigger)>()
        .iter(app.world())
        .find(|(_, trigger)| trigger.0 == combo)
        .map(|(entity, _)| entity)
        .expect("the combo has a trigger");
    let mut stack = vec![trigger];
    while let Some(entity) = stack.pop() {
        if let Some(text) = app.world().get::<Text>(entity) {
            return CANVAS_PRESETS
                .iter()
                .position(|preset| text.0 == format!("{} x {}", preset.x, preset.y))
                .unwrap_or_else(|| {
                    assert_eq!(text.0, "Custom");
                    CUSTOM_PRESET
                });
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter().rev());
        }
    }
    panic!("the combo draws a label");
}

fn resolution(app: &App, camera: Entity) -> Option<UVec2> {
    app.world()
        .get::<PixelCanvas>(camera)
        .map(|canvas| canvas.resolution)
}

#[test]
fn a_preset_pick_sets_the_resolution_and_undo_moves_the_combo_back() {
    let (mut app, camera) = app_with_canvas(PixelCanvas::new(320, 180));
    assert_eq!(shown(&mut app), 0);
    assert!(
        app.world_mut()
            .query::<Entity>()
            .iter(app.world())
            .any(
                |entity| jackdaw::inspector::field_edited_by(app.world(), entity)
                    .is_some_and(|(_, path)| path.starts_with("resolution"))
            ),
        "the reflected fields stay below for a custom size"
    );

    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    pick(&mut app, 2);
    assert_eq!(resolution(&app, camera), Some(UVec2::new(480, 270)));
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1
    );
    assert_eq!(shown(&mut app), 2);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(resolution(&app, camera), Some(UVec2::new(320, 180)));
    assert_eq!(shown(&mut app), 0, "the combo follows the undo");
}

/// Pick option `selected` of the card's preset combo, as the widget does: its
/// own index first, then the change.
fn pick(app: &mut App, selected: usize) {
    let entity = combo(app);
    app.world_mut()
        .entity_mut(entity)
        .insert(ComboBoxSelectedIndex(selected));
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity,
        selected,
        label: String::new(),
        value: None,
    });
    for _ in 0..3 {
        app.update();
    }
}

#[test]
fn a_refused_pick_leaves_the_combo_on_the_canvas_size() {
    let (mut app, entity) = app_with(PixelCanvas::new(320, 180), false);
    pick(&mut app, 2);
    assert_eq!(
        resolution(&app, entity),
        Some(UVec2::new(320, 180)),
        "the operator refuses an entity that is no 2D camera"
    );
    assert_eq!(shown(&mut app), 0, "the combo goes back to what it holds");
}

#[test]
fn custom_on_a_preset_size_snaps_back_to_the_preset() {
    let (mut app, camera) = app_with_canvas(PixelCanvas::new(384, 216));
    pick(&mut app, CUSTOM_PRESET);
    assert_eq!(resolution(&app, camera), Some(UVec2::new(384, 216)));
    assert_eq!(
        shown(&mut app),
        1,
        "Custom only names a size off the presets"
    );
}

#[test]
fn a_size_off_the_presets_reads_custom() {
    let (mut app, _camera) = app_with_canvas(PixelCanvas::new(256, 144));
    assert_eq!(shown(&mut app), CUSTOM_PRESET);
}

fn app_with_plain_camera() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    let camera = app
        .world_mut()
        .spawn((Name::new("Game Camera"), Camera2d, ChildOf(root)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), camera);
    jackdaw::selection::select_only(app.world_mut(), camera);
    for _ in 0..4 {
        app.update();
    }
    (app, camera)
}

fn canvas_toggle(app: &mut App, camera: Entity) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &PixelCanvasToggle)>()
        .iter(app.world())
        .filter(|(_, toggle)| toggle.source == camera)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one pixel canvas toggle");
    found[0]
}

fn flip(app: &mut App, camera: Entity, value: bool) {
    let source = canvas_toggle(app, camera);
    app.world_mut().trigger(ValueChange {
        source,
        value,
        is_final: true,
    });
    for _ in 0..4 {
        app.update();
    }
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn the_2d_camera_card_toggles_the_pixel_canvas_one_entry_each_way() {
    let (mut app, camera) = app_with_plain_camera();
    let toggle = canvas_toggle(&mut app, camera);
    assert!(
        app.world().get::<Checked>(toggle).is_none(),
        "off by default"
    );
    assert_eq!(resolution(&app, camera), None);

    let before = history_len(&app);
    flip(&mut app, camera, true);
    assert_eq!(resolution(&app, camera), Some(CANVAS_PRESETS[0]));
    assert_eq!(history_len(&app), before + 1);
    let toggle = canvas_toggle(&mut app, camera);
    assert!(app.world().get::<Checked>(toggle).is_some(), "shows on");
    assert_eq!(shown(&mut app), 0, "the canvas card shows its presets");

    flip(&mut app, camera, false);
    assert_eq!(resolution(&app, camera), None);
    assert_eq!(history_len(&app), before + 2);
}

#[test]
fn undo_after_toggling_on_clears_the_checkbox_and_a_matching_request_does_nothing() {
    let (mut app, camera) = app_with_plain_camera();
    let before = history_len(&app);
    flip(&mut app, camera, false);
    assert_eq!(history_len(&app), before, "already off: no call");

    flip(&mut app, camera, true);
    flip(&mut app, camera, true);
    assert_eq!(history_len(&app), before + 1, "already on: no second entry");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(resolution(&app, camera), None);
    let toggle = canvas_toggle(&mut app, camera);
    assert!(
        app.world().get::<Checked>(toggle).is_none(),
        "the checkbox follows the undo"
    );
}
