//! The collision mode of the Tileset window: its panel, painting presets on
//! tiles by dragging, clearing with the right button, and its overlay.

use crate::operators::tileset_dock::{
    control, handmade, input, opened, press, screen_rect, settle, texts, view,
};
use crate::operators::tileset_edit_ops::{collision_of, entries, on_disk, run};
use crate::operators::tileset_ops::warnings;

use bevy::prelude::*;
use jackdaw_editor_tile::tileset_collision::PhysicsLayerPick;
use jackdaw_editor_tile::tileset_dock::DockAction;
use jackdaw_editor_tile::tileset_modes::ModeId;
use jackdaw_editor_tile::zone_overlay::{ZoneOverlay, ZonePiece};
use jackdaw_feathers::combobox::ComboBoxChangeEvent;
use jackdaw_tileset::CollisionPreset;

/// A point of tile `tile` on the drawn sheet, from (0, 0) at its top left
/// to (1, 1), in the logical pixels `input.pointer` takes.
fn on_tile(app: &mut App, root: Entity, tile: u32, at: Vec2) -> Vec2 {
    let cell = control(app, root, DockAction::Tile(0, tile));
    let rect = screen_rect(app, cell);
    rect.min + at * rect.size()
}

fn drag(app: &mut App, from: Vec2, to: Vec2, button: &str) {
    input(
        app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    input(
        app,
        &format!(
            "input.pointer x={} y={} action=drag_to steps=24 button={button}",
            to.x, to.y
        ),
    );
}

/// The open dock under `root`, in the collision mode.
fn colliding(app: &mut App) -> Entity {
    let (_, root) = opened(app);
    press(app, root, DockAction::Mode(ModeId::COLLISION));
    root
}

/// The drawn pieces of the overlay of the dock under `root`.
fn pieces(app: &mut App, root: Entity) -> Vec<ZonePiece> {
    let overlays: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &ZoneOverlay)>()
        .iter(app.world())
        .filter(|(_, o)| o.root == root)
        .map(|(e, _)| e)
        .collect();
    let world = app.world();
    (overlays.into_iter())
        .flat_map(|o| world.get::<Children>(o).into_iter().flatten().copied())
        .filter_map(|e| world.get::<ZonePiece>(e).copied())
        .collect()
}

fn picked_shapes(found: &[ZonePiece]) -> usize {
    (found.iter())
        .filter(|p| matches!(p, ZonePiece::Shape { picked: true, .. }))
        .count()
}

#[test]
fn the_panel_shows_the_physics_layer_the_presets_and_the_trigger_toggle() {
    let (mut app, _dir) = handmade();
    let root = colliding(&mut app);
    let shown = texts(&mut app, root);
    for label in [
        "Solid",
        "Top half",
        "Slope up",
        "Traced from the art",
        "Trigger",
    ] {
        assert!(shown.iter().any(|t| t == label), "{label} in {shown:?}");
    }
    let picks = (app
        .world_mut()
        .query::<&PhysicsLayerPick>()
        .iter(app.world()))
    .filter(|p| p.root == root)
    .count();
    assert_eq!(picks, 1, "one physics layer combo");
    assert!(
        !shown.iter().any(|t| t == "Terrain sets"),
        "no terrain list"
    );

    press(
        &mut app,
        root,
        DockAction::CollisionPreset(CollisionPreset::SlopeDown),
    );
    press(&mut app, root, DockAction::CollisionTrigger);
    let brush = view(&app).collision;
    assert_eq!(brush.preset, CollisionPreset::SlopeDown);
    assert!(brush.trigger);
}

#[test]
fn a_drag_across_tiles_paints_the_picked_preset_as_one_undo_entry() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    press(
        &mut app,
        root,
        DockAction::CollisionPreset(CollisionPreset::BottomHalf),
    );
    let before = on_disk(&dir);
    let count = entries(&app);
    let from = on_tile(&mut app, root, 1, Vec2::new(0.5, 0.5));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.5, 0.5));
    drag(&mut app, from, to, "primary");

    let after = on_disk(&dir);
    for tile in 1..=3 {
        let shapes = collision_of(&after, tile);
        assert_eq!(shapes.len(), 1, "tile {tile}");
        assert_eq!(shapes[0].preset, CollisionPreset::BottomHalf);
    }
    assert!(collision_of(&after, 4).is_empty(), "past the stroke");
    assert_eq!(entries(&app), count + 1, "one undo entry for the stroke");
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    assert_eq!(picked_shapes(&pieces(&mut app, root)), 3);

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(on_disk(&dir), before, "undo restores the tileset exactly");
}

#[test]
fn a_right_drag_clears_and_other_presets_show_as_quiet_outlines() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    press(
        &mut app,
        root,
        DockAction::CollisionPreset(CollisionPreset::SlopeUp),
    );
    let from = on_tile(&mut app, root, 0, Vec2::new(0.5, 0.5));
    let to = on_tile(&mut app, root, 2, Vec2::new(0.5, 0.5));
    drag(&mut app, from, to, "primary");
    let picked = |p: &ZonePiece| matches!(p, ZonePiece::Segment { picked: true, .. });
    let quiet = |p: &ZonePiece| matches!(p, ZonePiece::Segment { picked: false, .. });
    let found = pieces(&mut app, root);
    assert_eq!(
        found.iter().filter(|p| picked(p)).count(),
        9,
        "three sides each"
    );

    press(
        &mut app,
        root,
        DockAction::CollisionPreset(CollisionPreset::Solid),
    );
    let found = pieces(&mut app, root);
    assert_eq!(found.iter().filter(|p| quiet(p)).count(), 9);
    assert_eq!(found.iter().filter(|p| picked(p)).count(), 0);

    let count = entries(&app);
    let from = on_tile(&mut app, root, 1, Vec2::new(0.5, 0.5));
    let to = on_tile(&mut app, root, 1, Vec2::new(0.7, 0.5));
    drag(&mut app, from, to, "secondary");
    let after = on_disk(&dir);
    assert!(collision_of(&after, 1).is_empty());
    assert_eq!(collision_of(&after, 0).len(), 1);
    assert_eq!(entries(&app), count + 1);
}

#[test]
fn a_new_physics_layer_is_shown_and_the_combo_picks_a_layer() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    press(&mut app, root, DockAction::AddPhysicsLayer);
    assert_eq!(on_disk(&dir).physics_layer_names().len(), 2);
    assert_eq!(view(&app).collision.layer, 1, "the new layer is shown");

    let from = on_tile(&mut app, root, 0, Vec2::new(0.5, 0.5));
    drag(&mut app, from, from + Vec2::X, "primary");
    let shapes = collision_of(&on_disk(&dir), 0);
    assert_eq!(shapes.iter().map(|s| s.layer).collect::<Vec<_>>(), [1]);

    let combo = (app.world_mut().query::<(Entity, &PhysicsLayerPick)>())
        .iter(app.world())
        .find(|(_, p)| p.root == root)
        .map(|(e, _)| e)
        .expect("a combo");
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo,
        selected: 0,
        label: String::new(),
        value: None,
    });
    settle(&mut app);
    assert_eq!(view(&app).collision.layer, 0);
    assert_eq!(
        picked_shapes(&pieces(&mut app, root)),
        0,
        "layer 0 is empty"
    );
}
