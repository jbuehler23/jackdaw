//! The `Sprite` card: the sprite's fields, the anchor as a preset combo with
//! a custom offset row, and the pixels per unit override, which writes
//! through `sprite.set_pixels_per_unit`.

use crate::util;

use bevy::{prelude::*, sprite::Anchor, ui_widgets::ValueChange};
use jackdaw::commands::CommandHistory;
use jackdaw::inspector::sprite_card::{
    CUSTOM_ANCHOR, SpriteAnchorCombo, SpriteCustomAnchor, SpritePpuApply, SpritePpuInput,
    SpriteYSortToggle,
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
        "Image",
        "Anchor",
        "Flip",
        "Custom size",
        "Size from PPU",
        "Color",
        "Draw order",
        "Rect",
        "Texture atlas",
        "Image mode",
    ];
    let found: Vec<&str> = labels
        .iter()
        .map(String::as_str)
        .filter(|label| wanted.contains(label))
        .collect();
    assert_eq!(found, wanted, "rows top to bottom: {labels:?}");
    assert!(
        labels
            .iter()
            .all(|label| !label.contains('_') && !label.starts_with(char::is_lowercase)),
        "every label is sentence case: {labels:?}"
    );
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

/// The row of the card body that holds `entity`.
fn row_of(app: &mut App, entity: Entity) -> Entity {
    let body = card_body(app);
    let mut at = entity;
    loop {
        let parent = app
            .world()
            .get::<ChildOf>(at)
            .map(ChildOf::parent)
            .expect("inside the card body");
        if parent == body {
            return at;
        }
        at = parent;
    }
}

fn field_widget(app: &mut App, type_path: &str, field_path: &str) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|&entity| {
            jackdaw::inspector::field_edited_by(app.world(), entity)
                == Some((type_path, field_path))
        })
        .unwrap_or_else(|| panic!("a widget editing {field_path}"))
}

#[test]
fn flip_x_and_flip_y_are_a_pair_in_one_flip_row() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let x = field_widget(&mut app, SPRITE, "flip_x");
    let y = field_widget(&mut app, SPRITE, "flip_y");
    let row = row_of(&mut app, x);
    assert_eq!(row, row_of(&mut app, y), "one row holds both");
    assert_eq!(first_text(&app, row).as_deref(), Some("Flip"));

    app.world_mut().trigger(ValueChange {
        source: y,
        value: true,
        is_final: true,
    });
    settle(&mut app);
    let flipped = app.world().get::<Sprite>(sprite).expect("a sprite");
    assert!(flipped.flip_y && !flipped.flip_x, "the Y box flips Y only");
}

/// The width of the label column of the row holding `entity`.
fn label_width(app: &mut App, entity: Entity) -> Val {
    let row = row_of(app, entity);
    let mut stack = vec![row];
    while let Some(at) = stack.pop() {
        if app.world().get::<Text>(at).is_some() {
            return app.world().get::<Node>(at).map_or(Val::Auto, |n| n.width);
        }
        if let Some(children) = app.world().get::<Children>(at) {
            stack.extend(children.iter().rev());
        }
    }
    panic!("the row has a label");
}

#[test]
fn the_colour_row_shares_the_label_column_of_the_other_rows() {
    let (mut app, _dir, _sprite) = app_with_sprite();
    let anchor = one::<SpriteAnchorCombo>(&mut app);
    let swatch = one::<bevy::feathers::controls::ColorSwatchValue>(&mut app);
    let column = label_width(&mut app, anchor);
    assert_eq!(column, Val::Px(jackdaw_feathers::tokens::FIELD_LABEL_WIDTH));
    assert_eq!(label_width(&mut app, swatch), column);
}

fn op_buttons(app: &mut App) -> Vec<(Entity, String)> {
    app.world_mut()
        .query::<(Entity, &jackdaw_feathers::button::ButtonOperatorCall)>()
        .iter(app.world())
        .map(|(entity, call)| (entity, call.id.to_string()))
        .collect()
}

#[test]
fn the_draw_order_row_has_buttons_for_the_page_keys_and_a_y_sort_toggle() {
    let (mut app, _dir, sprite) = app_with_sprite();
    let buttons = op_buttons(&mut app);
    let mut row = None;
    for id in ["sprite.z_down", "sprite.z_up"] {
        let (button, _) = buttons
            .iter()
            .find(|(_, call)| call == id)
            .unwrap_or_else(|| panic!("a {id} button: {buttons:?}"));
        let held = row_of(&mut app, *button);
        assert!(row.is_none_or(|row| row == held), "one row holds both");
        row = Some(held);
    }
    let row = row.expect("the draw order row");
    assert_eq!(first_text(&app, row).as_deref(), Some("Draw order"));

    let toggle = one::<SpriteYSortToggle>(&mut app);
    assert_eq!(row_of(&mut app, toggle), row);
    let depth = app.world().resource::<CommandHistory>().undo_stack.len();
    app.world_mut().trigger(ValueChange {
        source: toggle,
        value: true,
        is_final: true,
    });
    settle(&mut app);
    assert!(
        app.world()
            .get::<jackdaw_scene_types::YSort>(sprite)
            .is_some()
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len() - depth,
        1
    );
    let cards = jackdaw::inspector::component_cards_showing(app.world_mut());
    assert!(
        !cards
            .iter()
            .any(|path| path == jackdaw_scene_types::Y_SORT_TYPE_PATH),
        "the toggle stands for the Y-sort card: {cards:?}"
    );
    let toggle = one::<SpriteYSortToggle>(&mut app);
    assert!(app.world().get::<bevy::ui::Checked>(toggle).is_some());
}

fn z(app: &App, sprite: Entity) -> f32 {
    app.world()
        .get::<Transform>(sprite)
        .map_or(f32::NAN, |transform| transform.translation.z)
}

#[test]
fn the_bring_forward_button_moves_only_the_cards_sprite_even_with_shift_held() {
    let (mut app, _dir, shown) = app_with_sprite();
    let mut others = Vec::new();
    for _ in 0..2 {
        let added = app
            .world_mut()
            .operator("sprite.add")
            .param("path", SHEET)
            .call()
            .expect("sprite.add dispatches");
        assert_eq!(added, OperatorResult::Finished);
        settle(&mut app);
        others.push(
            app.world()
                .resource::<Selection>()
                .primary()
                .expect("the new sprite is selected"),
        );
    }
    app.world_mut().resource_mut::<Selection>().entities = vec![others[0], others[1], shown];
    settle(&mut app);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
    settle(&mut app);

    let before: Vec<f32> = others.iter().map(|&other| z(&app, other)).collect();
    let shown_before = z(&app, shown);
    let (button, _) = op_buttons(&mut app)
        .into_iter()
        .find(|(_, id)| id == "sprite.z_up")
        .expect("a bring forward button");
    assert!(
        app.world()
            .get::<bevy::ui::InteractionDisabled>(button)
            .is_none(),
        "Shift does not grey the button out"
    );
    app.world_mut()
        .trigger(bevy::ui_widgets::Activate { entity: button });
    settle(&mut app);
    assert_eq!(
        z(&app, shown),
        shown_before + 1.0,
        "the card's sprite moves"
    );
    let after: Vec<f32> = others.iter().map(|&other| z(&app, other)).collect();
    assert_eq!(after, before, "the other selected sprites stay");
}
