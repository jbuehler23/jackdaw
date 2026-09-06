//! The colour picker.
//!
//! The plane, the four channel sliders and the swatches are the feathers
//! colour controls; the picker supplies the popover shell, the hex
//! readout and the input rows around them.

use crate::util;

use bevy::feathers::controls::{
    ColorPlaneValue, ColorSwatchValue, FeathersColorPlane, FeathersColorSlider, FeathersColorSwatch,
};
use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;

use jackdaw_feathers::color_picker::{
    ColorPickerChangeEvent, ColorPickerProps, ColorPickerState, color_picker,
};

fn descendants(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(entity) = stack.pop() {
        out.push(entity);
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    out
}

fn with<C: Component>(world: &mut World, root: Entity) -> Vec<Entity> {
    descendants(world, root)
        .into_iter()
        .filter(|entity| world.get::<C>(*entity).is_some())
        .collect()
}

/// An inline picker with its content built.
fn app_with_picker(color: [f32; 4]) -> (App, Entity) {
    let mut app = util::editor_test_app();
    let picker = app
        .world_mut()
        .spawn(color_picker(
            ColorPickerProps::new().with_color(color).inline(),
        ))
        .id();
    for _ in 0..4 {
        app.update();
    }
    (app, picker)
}

/// Every control the picker draws is a feathers colour control.
#[test]
fn the_picker_is_built_from_the_feathers_colour_controls() {
    let (mut app, picker) = app_with_picker([1.0, 0.0, 0.0, 1.0]);

    assert_eq!(
        with::<FeathersColorPlane>(app.world_mut(), picker).len(),
        1,
        "one colour plane",
    );
    assert_eq!(
        with::<FeathersColorSlider>(app.world_mut(), picker).len(),
        4,
        "one slider per channel",
    );
    assert_eq!(
        with::<FeathersColorSwatch>(app.world_mut(), picker).len(),
        1,
        "and the preview swatch",
    );
}

/// The plane is handed the colour the picker was opened on: red on its x
/// axis, blue on its y, green as the fixed channel.
#[test]
fn the_plane_shows_the_colour_the_picker_holds() {
    let (mut app, picker) = app_with_picker([0.25, 0.5, 0.75, 1.0]);

    let plane = with::<FeathersColorPlane>(app.world_mut(), picker)[0];
    let value = app
        .world()
        .get::<ColorPlaneValue>(plane)
        .expect("the plane carries a value")
        .0;
    assert!(
        (value - Vec3::new(0.25, 0.75, 0.5)).length() < 1e-3,
        "the plane reads red, blue and a fixed green; got {value:?}",
    );
}

/// Dragging the plane writes the colour back and tells the field.
#[test]
fn a_drag_on_the_plane_writes_the_colour() {
    let (mut app, picker) = app_with_picker([0.0, 0.5, 0.0, 1.0]);
    let plane = with::<FeathersColorPlane>(app.world_mut(), picker)[0];

    let changed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<[f32; 4]>::new()));
    let seen = changed.clone();
    app.world_mut()
        .add_observer(move |change: On<ColorPickerChangeEvent>| {
            seen.lock()
                .expect("no other thread holds it")
                .push(change.color);
        });

    app.world_mut().trigger(ValueChange {
        source: plane,
        value: Vec2::new(1.0, 1.0),
        is_final: true,
    });
    app.update();
    app.update();

    let state = app
        .world()
        .get::<ColorPickerState>(picker)
        .expect("the picker holds its state")
        .clone();
    let rgba = state.to_rgba();
    assert!(
        (rgba[0] - 1.0).abs() < 1e-3 && (rgba[2] - 1.0).abs() < 1e-3,
        "the drag wrote red and blue; got {rgba:?}",
    );

    assert!(
        !changed.lock().expect("no other thread holds it").is_empty(),
        "and the field was told",
    );

    let swatch = with::<FeathersColorSwatch>(app.world_mut(), picker)[0];
    assert_eq!(
        app.world()
            .get::<ColorSwatchValue>(swatch)
            .expect("the swatch carries a value")
            .0,
        Color::Srgba(state.to_srgba()),
        "and the preview swatch followed",
    );
}
