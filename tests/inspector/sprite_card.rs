//! The `Sprite` card: the sprite's fields, the anchor as a preset combo with
//! a custom offset row, and the pixels per unit override, which writes
//! through `sprite.set_pixels_per_unit`.

use crate::util;

use bevy::{prelude::*, sprite::Anchor, ui_widgets::ValueChange};
use jackdaw::commands::CommandHistory;
use jackdaw::inspector::sprite_card::{
    CUSTOM_ANCHOR, SpriteAnchorCombo, SpriteCustomAnchor, SpritePpuApply, SpritePpuInput,
};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::combobox::ComboBoxChangeEvent;
use jackdaw_feathers::number_input::ScrubNumberInputValue;

const SPRITE: &str = "bevy_sprite::sprite::Sprite";
const ANCHOR: &str = "bevy_sprite::sprite::Anchor";
const SHEET: &str = "overworld.png";

/// A selected sprite with the inspector open: of the overworld sheet, loaded,
/// or with no `path` a placeholder square.
fn app_with(path: Option<&'static str>) -> (App, tempfile::TempDir, Entity) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::copy(util::OVERWORLD_SHEET, dir.path().join(SHEET)).expect("copy the sheet");
    let mut app = util::editor_test_app_reading(dir.path());
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));

    jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    app.update();
    let mut call = app.world_mut().operator("sprite.add");
    if let Some(path) = path {
        call = call.param("path", path);
    }
    let added = call.call().expect("sprite.add dispatches");
    assert_eq!(added, OperatorResult::Finished);
    app.update();
    let entity = app
        .world()
        .resource::<Selection>()
        .primary()
        .expect("the new sprite is selected");
    let image = app
        .world()
        .get::<Sprite>(entity)
        .expect("a sprite")
        .image
        .clone();
    if path.is_some() {
        util::update_until(
            &mut app,
            "the sheet to load",
            std::time::Duration::from_secs(20),
            |app| {
                app.world()
                    .resource::<Assets<Image>>()
                    .get(&image)
                    .is_some()
            },
        );
    }
    for _ in 0..4 {
        app.update();
    }
    (app, dir, entity)
}

fn app_with_sprite() -> (App, tempfile::TempDir, Entity) {
    app_with(Some(SHEET))
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

fn has_field_widget(app: &mut App, type_path: &str, field_path: &str) -> bool {
    all_entities(app).into_iter().any(|entity| {
        jackdaw::inspector::field_edited_by(app.world(), entity)
            .is_some_and(|(ty, path)| ty == type_path && path.starts_with(field_path))
    })
}

fn one<T: Component>(app: &mut App) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<T>>()
        .iter(app.world())
        .collect();
    assert_eq!(found.len(), 1, "one {}", std::any::type_name::<T>());
    found[0]
}

fn pick(app: &mut App, combo: Entity, selected: usize) {
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo,
        selected,
        label: String::new(),
        value: None,
    });
    for _ in 0..3 {
        app.update();
    }
}

fn custom_row_shown(app: &mut App) -> bool {
    let row = one::<SpriteCustomAnchor>(app);
    app.world()
        .get::<Node>(row)
        .is_some_and(|node| node.display != Display::None)
}

#[test]
fn the_sprite_card_offers_every_sprite_control() {
    let (mut app, _dir, _sprite) = app_with_sprite();
    assert!(
        all_entities(&mut app).into_iter().any(|entity| {
            jackdaw::inspector::asset_field_shown_by(app.world(), entity)
                .is_some_and(|(_, field)| field == "image")
        }),
        "the card shows the image as an asset row"
    );
    assert!(
        all_entities(&mut app).into_iter().any(|entity| {
            jackdaw::inspector::enum_field_shown_by(app.world(), entity)
                == Some((SPRITE, "custom_size"))
        }),
        "the card offers custom_size"
    );
    for field in ["flip_x", "flip_y"] {
        assert!(
            has_field_widget(&mut app, SPRITE, field),
            "the card edits `{field}`"
        );
    }
    one::<SpriteAnchorCombo>(&mut app);
    one::<SpritePpuInput>(&mut app);
    one::<SpritePpuApply>(&mut app);
    assert!(
        !custom_row_shown(&mut app),
        "a preset anchor hides the offset"
    );
}

#[test]
fn a_sprite_has_no_anchor_card_of_its_own() {
    let (mut app, _dir, _sprite) = app_with_sprite();
    let cards = jackdaw::inspector::component_cards_showing(app.world_mut());
    assert!(cards.iter().any(|path| path == SPRITE), "{cards:?}");
    assert!(!cards.iter().any(|path| path == ANCHOR), "{cards:?}");
}

#[test]
fn choosing_an_anchor_preset_commits_it_and_undo_restores_it() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let depth = app.world().resource::<CommandHistory>().undo_stack.len();
    let combo = one::<SpriteAnchorCombo>(&mut app);
    let bottom_left = jackdaw::inspector::sprite_card::anchor_index(Anchor::BOTTOM_LEFT);
    pick(&mut app, combo, bottom_left);
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor::BOTTOM_LEFT)
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len() - depth,
        1
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    let combo = one::<SpriteAnchorCombo>(&mut app);
    assert_eq!(
        app.world()
            .get::<jackdaw_feathers::combobox::ComboBoxSelectedIndex>(combo)
            .map(|index| index.0),
        Some(jackdaw::inspector::sprite_card::anchor_index(
            Anchor::CENTER
        )),
        "the combo follows the undo"
    );
}

#[test]
fn custom_shows_the_offset_row() {
    let (mut app, _dir, sprite) = app_with_sprite();
    assert!(has_field_widget(&mut app, ANCHOR, ".0"));
    let combo = one::<SpriteAnchorCombo>(&mut app);
    pick(&mut app, combo, CUSTOM_ANCHOR);
    assert!(custom_row_shown(&mut app));
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
}

fn settle(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

fn shown_index(app: &mut App, combo: Entity) -> Option<usize> {
    app.world()
        .get::<jackdaw_feathers::combobox::ComboBoxSelectedIndex>(combo)
        .map(|index| index.0)
}

#[test]
fn the_custom_row_stays_while_an_offset_edit_passes_a_preset() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let combo = one::<SpriteAnchorCombo>(&mut app);
    pick(&mut app, combo, CUSTOM_ANCHOR);
    for axis in ["x", "y"] {
        let input = all_entities(&mut app)
            .into_iter()
            .find(|&entity| {
                jackdaw::inspector::field_edited_by(app.world(), entity)
                    == Some((ANCHOR, &format!(".0.{axis}")))
            })
            .unwrap_or_else(|| panic!("an input for the offset {axis}"));
        app.world_mut().trigger(ValueChange {
            source: input,
            value: -0.5_f64,
            is_final: true,
        });
        settle(&mut app);
    }
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor::BOTTOM_LEFT)
    );
    assert!(custom_row_shown(&mut app), "the offset row stays open");
    let combo = one::<SpriteAnchorCombo>(&mut app);
    assert_eq!(shown_index(&mut app, combo), Some(CUSTOM_ANCHOR));
}

/// The first text under `entity`, depth first in child order.
fn first_text(app: &App, entity: Entity) -> Option<String> {
    if let Some(text) = app.world().get::<Text>(entity) {
        return Some(text.0.trim_end_matches(':').to_string());
    }
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(entity)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    children
        .into_iter()
        .find_map(|child| first_text(app, child))
}

/// The body of the Sprite card, which holds its rows.
fn card_body(app: &mut App) -> Entity {
    let combo = one::<SpriteAnchorCombo>(app);
    app.world()
        .get::<ChildOf>(combo)
        .and_then(|control| app.world().get::<ChildOf>(control.parent()))
        .and_then(|row| app.world().get::<ChildOf>(row.parent()))
        .map(ChildOf::parent)
        .expect("the anchor row sits in the card body")
}

/// The labels of the card's rows, top to bottom.
fn row_labels(app: &mut App) -> Vec<String> {
    let body = card_body(app);
    app.world()
        .get::<Children>(body)
        .expect("rows")
        .iter()
        .filter_map(|row| first_text(app, row))
        .collect()
}

#[test]
fn every_row_is_labelled_and_in_sprite_order() {
    let (mut app, _dir, _sprite) = app_with_sprite();
    let labels = row_labels(&mut app);
    let wanted = [
        "image",
        "anchor",
        "flip_x",
        "flip_y",
        "custom_size",
        "size from ppu",
        "color",
        "rect",
        "texture_atlas",
        "image_mode",
    ];
    let found: Vec<&str> = labels
        .iter()
        .map(String::as_str)
        .filter(|label| wanted.contains(label))
        .collect();
    assert_eq!(found, wanted, "rows top to bottom: {labels:?}");
    let text = jackdaw::inspector::component_card_text(app.world_mut(), SPRITE);
    assert!(
        !text.iter().any(|line| line == "0"),
        "no bare tuple index label: {text:?}"
    );
}

fn custom_size(app: &App, sprite: Entity) -> Option<Vec2> {
    app.world()
        .get::<Sprite>(sprite)
        .and_then(|s| s.custom_size)
}

/// Type `ppu` into the size row and press Apply.
fn apply_ppu(app: &mut App, ppu: f64) {
    let input = one::<SpritePpuInput>(app);
    app.world_mut().trigger(ValueChange {
        source: input,
        value: ppu,
        is_final: true,
    });
    settle(app);
    let apply = one::<SpritePpuApply>(app);
    app.world_mut().trigger(ButtonClickEvent { entity: apply });
    settle(app);
}

fn ppu_shown(app: &mut App) -> f64 {
    let input = one::<SpritePpuInput>(app);
    match app.world().get::<ScrubNumberInputValue>(input) {
        Some(ScrubNumberInputValue::F64(value)) => *value,
        other => panic!("an f64 value, not {other:?}"),
    }
}

#[test]
fn apply_sizes_the_sprite_from_the_typed_ppu_and_keeps_no_state() {
    let (mut app, _dir, sprite) = app_with_sprite();
    assert_eq!(ppu_shown(&mut app), 1.0, "the row starts at one");
    let depth = app.world().resource::<CommandHistory>().undo_stack.len();
    apply_ppu(&mut app, 16.0);
    assert_eq!(custom_size(&app, sprite), Some(Vec2::new(27.0, 65.0)));
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len() - depth,
        1
    );
    reselect(&mut app, sprite);
    assert_eq!(ppu_shown(&mut app), 1.0, "a rebuild shows no stored ppu");
}

#[test]
fn the_row_starts_at_the_project_ppu() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let set = app
        .world_mut()
        .operator("project.set_pixels_per_unit")
        .param("value", 16.0)
        .call()
        .expect("project.set_pixels_per_unit dispatches");
    assert_eq!(set, OperatorResult::Finished);
    settle(&mut app);
    reselect(&mut app, sprite);
    assert_eq!(ppu_shown(&mut app), 16.0);
}

#[test]
fn apply_takes_the_operators_whole_range() {
    let (mut app, _dir, sprite) = app_with_sprite();
    for ppu in [0.001_f64, 50_000.0] {
        apply_ppu(&mut app, ppu);
        assert_eq!(
            custom_size(&app, sprite),
            Some(Vec2::new(432.0, 1040.0) / ppu as f32),
            "{ppu} is applied unclamped"
        );
    }
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(app);
}

fn type_offset(app: &mut App, axis: &str, value: f64) {
    let input = all_entities(app)
        .into_iter()
        .find(|&entity| {
            jackdaw::inspector::field_edited_by(app.world(), entity)
                == Some((ANCHOR, &format!(".0.{axis}")))
        })
        .unwrap_or_else(|| panic!("an input for the offset {axis}"));
    app.world_mut().trigger(ValueChange {
        source: input,
        value,
        is_final: true,
    });
    settle(app);
}

#[test]
fn undoing_a_custom_offset_back_to_a_preset_shows_the_preset() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let combo = one::<SpriteAnchorCombo>(&mut app);
    pick(&mut app, combo, CUSTOM_ANCHOR);
    type_offset(&mut app, "x", 0.3);
    assert_eq!(app.world().get::<Anchor>(sprite).map(|a| a.0.x), Some(0.3));
    undo(&mut app);
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    let combo = one::<SpriteAnchorCombo>(&mut app);
    assert_eq!(
        shown_index(&mut app, combo),
        Some(jackdaw::inspector::sprite_card::anchor_index(
            Anchor::CENTER
        ))
    );
    assert!(!custom_row_shown(&mut app), "a preset hides the offset row");
}

#[test]
fn a_custom_anchor_written_elsewhere_switches_the_combo_to_custom() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let _ = app
        .world_mut()
        .operator("component.set")
        .param("entity", sprite)
        .param("type_path", ANCHOR)
        .param("field", ".0")
        .param("value", "[0.25, 0.1]")
        .call()
        .expect("component.set dispatches");
    settle(&mut app);
    let combo = one::<SpriteAnchorCombo>(&mut app);
    assert_eq!(shown_index(&mut app, combo), Some(CUSTOM_ANCHOR));
    assert!(custom_row_shown(&mut app));
}

#[test]
fn a_read_only_remote_sprite_ignores_a_preset_pick() {
    let (mut app, _dir, sprite) = app_with_sprite();
    app.world_mut()
        .entity_mut(sprite)
        .insert(jackdaw::remote::entity_browser::RemoteEntityProxy { remote_bits: 7 });
    let depth = app.world().resource::<CommandHistory>().undo_stack.len();
    let combo = one::<SpriteAnchorCombo>(&mut app);
    pick(&mut app, combo, 0);
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        depth
    );
}

/// Select nothing, then `sprite` again, so the inspector builds its card anew.
fn reselect(app: &mut App, sprite: Entity) {
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle(app);
    jackdaw::selection::select_only(app.world_mut(), sprite);
    settle(app);
}

#[test]
fn apply_sizes_only_the_sprite_the_card_shows() {
    let (mut app, _dir, shown) = app_with_sprite();
    let added = app
        .world_mut()
        .operator("sprite.add")
        .param("path", SHEET)
        .call()
        .expect("sprite.add dispatches");
    assert_eq!(added, OperatorResult::Finished);
    settle(&mut app);
    let other = app
        .world()
        .resource::<Selection>()
        .primary()
        .expect("the second sprite is selected");
    app.world_mut().resource_mut::<Selection>().entities = vec![other, shown];
    settle(&mut app);
    apply_ppu(&mut app, 16.0);
    assert_eq!(custom_size(&app, shown), Some(Vec2::new(27.0, 65.0)));
    assert_eq!(custom_size(&app, other), None, "the other selected sprite");
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    settle(app);
}

#[test]
fn a_script_write_shows_its_preset_even_while_the_combo_reads_custom() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let combo = one::<SpriteAnchorCombo>(&mut app);
    pick(&mut app, combo, CUSTOM_ANCHOR);
    let _ = app
        .world_mut()
        .operator("field.set")
        .param("entity", sprite)
        .param("type_path", ANCHOR)
        .param("field", ".0")
        .param("value", "[-0.5, -0.5]")
        .call()
        .expect("field.set dispatches");
    settle(&mut app);
    let combo = one::<SpriteAnchorCombo>(&mut app);
    assert_eq!(
        shown_index(&mut app, combo),
        Some(jackdaw::inspector::sprite_card::anchor_index(
            Anchor::BOTTOM_LEFT
        ))
    );
    assert!(!custom_row_shown(&mut app));
}

#[test]
fn a_redo_shows_its_preset_even_while_the_combo_reads_custom() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let combo = one::<SpriteAnchorCombo>(&mut app);
    let bottom_left = jackdaw::inspector::sprite_card::anchor_index(Anchor::BOTTOM_LEFT);
    pick(&mut app, combo, bottom_left);
    undo(&mut app);
    assert_eq!(app.world().get::<Anchor>(sprite), Some(&Anchor::CENTER));
    let combo = one::<SpriteAnchorCombo>(&mut app);
    pick(&mut app, combo, CUSTOM_ANCHOR);
    redo(&mut app);
    assert_eq!(
        app.world().get::<Anchor>(sprite),
        Some(&Anchor::BOTTOM_LEFT)
    );
    let combo = one::<SpriteAnchorCombo>(&mut app);
    assert_eq!(shown_index(&mut app, combo), Some(bottom_left));
    assert!(!custom_row_shown(&mut app));
}
