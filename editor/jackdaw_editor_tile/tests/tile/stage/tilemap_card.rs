//! The tilemap's inspector card: its Edit tiles button, one aligned row per
//! layer, word labels, the cell size on one row, and the hint on a terrain
//! layer whose tileset has nothing to paint with.

use crate::stage::tilemap_render::{TILESET, project_app, settle, spawn_map};

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::ui_widgets::Activate;
use jackdaw_api::prelude::*;
use jackdaw_editor_tile::tile_tools::TileEditMode;
use jackdaw_editor_tile::tilemap_card::{
    LayerCombo, LayerNameField, TerrainHintButton, TilemapAddLayerButton, TilemapEditButton,
    TilemapLayerRow,
};
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::field_row::FieldRowNode;
use jackdaw_feathers::number_input::ScrubNumberInputValue;
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_panels::tree::DockTree;
use jackdaw_scene_types::{
    LayerTerrainMatch, PropertyValue, Tilemap, TilemapLayer, TilemapLayerKind,
};
use jackdaw_tilemap::TilemapData;

fn call(app: &mut App, id: &str, params: &[(&'static str, PropertyValue)]) -> OperatorResult {
    let mut op = app.world_mut().operator(id.to_string());
    for (name, value) in params {
        op = op.param(*name, value.clone());
    }
    let result = op.call().expect("dispatches");
    settle(app);
    result
}

fn text(value: &str) -> PropertyValue {
    PropertyValue::from(value.to_string())
}

/// A map of the overworld tileset with a second, tile layer, its card open.
fn card_with_two_layers() -> (App, tempfile::TempDir, Entity) {
    let (mut app, dir) = project_app();
    jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    let result = call(&mut app, "tilemap.create", &[("tileset", text(TILESET))]);
    assert_eq!(result, OperatorResult::Finished);
    let map = (app.world_mut())
        .query_filtered::<Entity, With<Tilemap>>()
        .single(app.world())
        .expect("one map");
    let result = call(
        &mut app,
        "tilemap.layer.add",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("kind", text("tile")),
            ("name", text("Props")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished);
    jackdaw_editor_core::selection::select_only(app.world_mut(), map);
    settle(&mut app);
    (app, dir, map)
}

fn screen_rect(app: &App, entity: Entity) -> Rect {
    let node = app.world().get::<ComputedNode>(entity).expect("laid out");
    let at = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("placed");
    let scale = node.inverse_scale_factor();
    Rect::from_center_size(at.translation * scale, node.size() * scale)
}

fn shown(app: &App, entity: Entity) -> bool {
    app.world()
        .get::<Node>(entity)
        .is_some_and(|node| node.display != Display::None)
}

fn edit_button(app: &mut App, exit: bool) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &TilemapEditButton)>()
        .iter(app.world())
        .filter(|(_, button)| button.exit == exit)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one edit button with exit {exit}");
    found[0]
}

fn rows(app: &mut App) -> Vec<(Entity, TilemapLayerRow)> {
    let mut found: Vec<_> = app
        .world_mut()
        .query::<(Entity, &TilemapLayerRow)>()
        .iter(app.world())
        .map(|(entity, row)| (entity, *row))
        .collect();
    found.sort_by_key(|(_, row)| row.index);
    found
}

/// Every text under the card body the add-layer button sits in.
fn card_texts(app: &mut App) -> Vec<String> {
    let add = app
        .world_mut()
        .query_filtered::<Entity, With<TilemapAddLayerButton>>()
        .single(app.world())
        .expect("one add button");
    let body = app
        .world()
        .get::<ChildOf>(add)
        .expect("in the body")
        .parent();
    let mut texts = Vec::new();
    let mut stack = vec![body];
    while let Some(entity) = stack.pop() {
        if let Some(t) = app.world().get::<Text>(entity) {
            texts.push(t.0.clone());
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    texts
}

#[test]
fn the_edit_tiles_button_enters_and_leaves_tile_edit_mode() {
    let (mut app, _dir, map) = card_with_two_layers();
    let enter = edit_button(&mut app, false);
    let exit = edit_button(&mut app, true);
    assert!(shown(&app, enter) && !shown(&app, exit));
    let tip = app
        .world()
        .get::<Tooltip>(enter)
        .cloned()
        .unwrap_or_default();
    assert!(tip.keybind.contains('1'), "{tip:?}");

    app.world_mut().trigger(Activate { entity: enter });
    settle(&mut app);
    assert_eq!(app.world().resource::<TileEditMode>().map, Some(map));
    let (enter, exit) = (edit_button(&mut app, false), edit_button(&mut app, true));
    assert!(shown(&app, exit) && !shown(&app, enter));

    app.world_mut().trigger(Activate { entity: exit });
    settle(&mut app);
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
    let enter = edit_button(&mut app, false);
    assert!(shown(&app, enter));
}

#[test]
fn layer_rows_line_up_and_the_painted_one_is_highlighted() {
    let (mut app, _dir, map) = card_with_two_layers();
    let result = call(
        &mut app,
        "tilemap.edit_mode",
        &[("layer", PropertyValue::from(1_i64))],
    );
    assert_eq!(result, OperatorResult::Finished);
    let rows = rows(&mut app);
    assert_eq!(rows.len(), 2);
    let (first, second) = (screen_rect(&app, rows[0].0), screen_rect(&app, rows[1].0));
    assert_eq!(first.height(), second.height(), "rows share one height");
    assert_eq!(first.min.x, second.min.x, "rows share one left edge");
    assert!(first.height() <= 2.0 * 30.0, "a compact row: {first:?}");

    let names: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<LayerNameField>>()
        .iter(app.world())
        .collect();
    assert_eq!(names.len(), 2);
    let lefts: Vec<f32> = names.iter().map(|n| screen_rect(&app, *n).min.x).collect();
    assert_eq!(lefts[0], lefts[1], "name fields share one left edge");
    let kinds: Vec<f32> = (app.world_mut())
        .query::<(Entity, &LayerCombo)>()
        .iter(app.world())
        .filter(|(_, combo)| combo.param == "kind")
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|kind| screen_rect(&app, kind).min.x)
        .collect();
    assert!(
        kinds.iter().all(|left| *left == lefts[0]),
        "the kind combos start under the names: {kinds:?} {lefts:?}"
    );

    let background = |row: Entity| app.world().get::<BackgroundColor>(row).map(|b| b.0);
    assert_eq!(background(rows[1].0), Some(tokens::SELECTED_BG));
    assert_ne!(background(rows[0].0), Some(tokens::SELECTED_BG));
    assert_eq!(rows[1].1.map, map);
}

#[test]
fn the_card_labels_are_words_and_the_cell_size_is_one_row() {
    let (mut app, _dir, _map) = card_with_two_layers();
    let texts = card_texts(&mut app);
    for label in ["Cell size", "Bounds", "Data file"] {
        assert!(texts.iter().any(|t| t == label), "{label} in {texts:?}");
    }
    assert!(
        !texts.iter().any(|t| t.contains('_')),
        "no field names: {texts:?}"
    );

    let label = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .find(|(_, t)| t.0 == "Cell size")
        .map(|(entity, _)| entity)
        .expect("a cell size label");
    let row = app
        .world()
        .get::<ChildOf>(label)
        .expect("in a row")
        .parent();
    let mut axes = Vec::new();
    let mut stack = vec![row];
    while let Some(entity) = stack.pop() {
        if app.world().get::<ScrubNumberInputValue>(entity).is_some() {
            axes.push(entity);
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    assert_eq!(axes.len(), 2, "X and Y beside the label: {axes:?}");
    let (x, y) = (screen_rect(&app, axes[0]), screen_rect(&app, axes[1]));
    assert_eq!(x.center().y, y.center().y, "X and Y share one row");
    let label_rect = screen_rect(&app, label);
    assert!(
        label_rect.max.y <= x.min.y,
        "the label sits above X and Y as on a Vec2 row: {label_rect:?} {x:?}"
    );
    assert!(
        app.world().get::<FieldRowNode>(row).is_none(),
        "drawn as a Vec2 row, not a label and control field row"
    );
    for axis in axes {
        assert!(
            matches!(
                app.world().get::<ScrubNumberInputValue>(axis),
                Some(ScrubNumberInputValue::I64(16))
            ),
            "whole cells"
        );
    }
}

#[test]
fn a_terrain_layer_without_terrain_sets_explains_itself() {
    let (mut app, _dir) = project_app();
    jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    let result = call(
        &mut app,
        "tileset.import",
        &[
            ("paths", text("overworld.png")),
            ("out", text("plain")),
            ("setup", text("none")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished);
    let layer = TilemapLayer {
        name: "Ground".to_string(),
        tileset: "plain.tileset.bsn".to_string(),
        kind: TilemapLayerKind::Terrain,
        terrain_match: LayerTerrainMatch::Auto,
    };
    let map = spawn_map(&mut app, vec![layer], TilemapData::default());
    jackdaw_editor_core::selection::select_only(app.world_mut(), map);
    settle(&mut app);

    let texts = card_texts(&mut app);
    assert!(
        texts.iter().any(|t| t.contains("no terrain sets yet")),
        "{texts:?}"
    );
    let hint = app
        .world_mut()
        .query_filtered::<Entity, With<TerrainHintButton>>()
        .single(app.world())
        .expect("one hint button");
    app.world_mut().trigger(Activate { entity: hint });
    settle(&mut app);
    let opened = (app.world().resource::<DockTree>().tabs())
        .any(|(_, tab)| tab.payload.as_deref() == Some("plain.tileset.bsn"));
    assert!(opened, "the hint opens the tileset");
}

#[test]
fn a_terrain_layer_with_painted_sets_shows_no_hint() {
    let (mut app, _dir, _map) = card_with_two_layers();
    let hints = app
        .world_mut()
        .query_filtered::<Entity, With<TerrainHintButton>>()
        .iter(app.world())
        .count();
    assert_eq!(hints, 0);
}

#[test]
fn the_edit_tiles_button_names_the_cards_map() {
    let (mut app, _dir, map) = card_with_two_layers();
    let enter = edit_button(&mut app, false);
    let call = app
        .world()
        .get::<ButtonOperatorCall>(enter)
        .expect("an operator button");
    assert!(
        call.params
            .iter()
            .any(|(name, value)| name == "entity" && *value == PropertyValue::Entity(map)),
        "{call}"
    );
}

/// The editable text of the card's Data file field.
fn data_file_text(app: &mut App) -> Entity {
    let label = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .find(|(_, t)| t.0 == "Data file")
        .map(|(entity, _)| entity)
        .expect("a data file label");
    let row = app
        .world()
        .get::<ChildOf>(label)
        .expect("in a row")
        .parent();
    let mut stack = vec![row];
    while let Some(entity) = stack.pop() {
        if app
            .world()
            .get::<bevy::text::EditableText>(entity)
            .is_some()
        {
            return entity;
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    panic!("the data file is an editable field");
}

fn shown_text(app: &App, text: Entity) -> String {
    (app.world().get::<bevy::text::EditableText>(text))
        .map(|editable| editable.value().to_string())
        .unwrap_or_default()
}

fn data_path(app: &App, map: Entity) -> String {
    app.world()
        .get::<Tilemap>(map)
        .expect("a map")
        .data_path
        .clone()
}

#[test]
fn the_data_file_is_edited_in_the_card_and_follows_undo() {
    use bevy::text::TextEdit;

    let (mut app, _dir, map) = card_with_two_layers();
    let before = data_path(&app, map);
    let text = data_file_text(&mut app);
    assert_eq!(shown_text(&app, text), before);
    {
        let mut editable = app
            .world_mut()
            .get_mut::<bevy::text::EditableText>(text)
            .expect("editable");
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert("other.jdtiles".into()));
    }
    app.update();
    app.world_mut()
        .trigger(bevy::input_focus::FocusLost { entity: text });
    settle(&mut app);
    assert_eq!(data_path(&app, map), "other.jdtiles");

    let result = call(&mut app, "history.undo", &[]);
    assert_eq!(result, OperatorResult::Finished);
    assert_eq!(data_path(&app, map), before);
    let text = data_file_text(&mut app);
    assert_eq!(
        shown_text(&app, text),
        before,
        "the field shows the undone path"
    );
}

#[test]
fn saving_several_maps_keeps_the_open_cards_data_file_current() {
    let (mut app, _dir, first) = card_with_two_layers();
    let result = call(&mut app, "tilemap.create", &[("tileset", text(TILESET))]);
    assert_eq!(result, OperatorResult::Finished);
    jackdaw_editor_core::selection::select_only(app.world_mut(), first);
    settle(&mut app);
    let before = data_path(&app, first);

    jackdaw_editor_core::scene_io::retarget_active_scene(app.world_mut(), "farm.bsn")
        .expect("retarget");
    assert!(
        jackdaw_editor_core::scene_io::save_scene(app.world_mut()),
        "saved"
    );
    settle(&mut app);
    let after = data_path(&app, first);
    assert_ne!(after, before, "the save moved the cells");
    let text = data_file_text(&mut app);
    assert_eq!(shown_text(&app, text), after);
}
