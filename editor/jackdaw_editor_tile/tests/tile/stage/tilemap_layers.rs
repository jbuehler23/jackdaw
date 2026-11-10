//! Making a tilemap from a tileset and managing its layers: `tilemap.create`,
//! the layer operators with one exact undo step each over the scene and the
//! cells, and the Layers section of the tilemap's inspector card.

use crate::stage::tilemap_render::{TILESET, key_of, project_app, settle};

use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_api_internal::operator::OperatorWarnings;
use jackdaw_commands::CommandHistory;
use jackdaw_editor_core::selection::Selection;
use jackdaw_editor_tile::tile_tools::TileEditMode;
use jackdaw_editor_tile::tilemap::TilemapDataStore;
use jackdaw_editor_tile::tilemap_card::{
    LayerCombo, LayerNameField, TilemapAddLayerButton, TilemapLayerButton,
};
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, ComboBoxSelectedIndex};
use jackdaw_feathers::text_edit::TextEditCommitEvent;
use jackdaw_scene_types::{LayerTerrainMatch, PropertyValue, Tilemap, TilemapLayerKind};
use jackdaw_tilemap::TilemapData;
use jackdaw_tileset::Tileset;

fn call(app: &mut App, id: &str, params: &[(&'static str, PropertyValue)]) -> OperatorResult {
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
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

fn entries(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn warnings(app: &App) -> String {
    (app.world().get_resource::<OperatorWarnings>()).map_or(String::new(), |w| w.0.join("\n"))
}

fn maps(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .iter(app.world())
        .collect()
}

fn tilemap(app: &App, map: Entity) -> Tilemap {
    app.world().get::<Tilemap>(map).cloned().expect("a tilemap")
}

fn stored(app: &mut App, map: Entity) -> TilemapData {
    let key = key_of(app, map);
    (app.world().resource::<TilemapDataStore>().get(&key))
        .cloned()
        .expect("the map has cells")
}

fn tileset(app: &App) -> Tileset {
    (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset")
}

fn available(app: &mut App, id: &str) -> bool {
    app.world_mut()
        .operator(id.to_string())
        .is_available()
        .unwrap_or(false)
}

fn reason(app: &mut App, id: &str) -> String {
    let mut ops = app.world_mut().query::<&OperatorEntity>();
    let op = (ops.iter(app.world()))
        .find(|op| op.id() == id)
        .expect("registered");
    op.unavailable_reason(app.world()).unwrap_or_default()
}

/// A project with the overworld tileset, a 2D scene open, and a map made
/// from the tileset.
fn app_with_map() -> (App, tempfile::TempDir, Entity, Entity) {
    let (mut app, dir) = project_app();
    let root = jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    let result = call(&mut app, "tilemap.create", &[("tileset", text(TILESET))]);
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    let map = maps(&mut app)[0];
    (app, dir, root, map)
}

fn paint(app: &mut App, map: Entity, layer: &str, kind: &str, value: &str) {
    let result = call(
        app,
        "tilemap.set_cells",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", text(layer)),
            ("kind", text(kind)),
            ("cells", text("0,0,3,3")),
            ("value", text(value)),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(app));
}

fn add_tile_layer(app: &mut App, map: Entity, name: &str) {
    let result = call(
        app,
        "tilemap.layer.add",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("kind", text("tile")),
            ("name", text(name)),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(app));
}

#[test]
fn create_makes_a_selected_map_of_the_tileset_as_one_undo_step() {
    let (mut app, _dir) = project_app();
    let root = jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    let count = entries(&app);
    let result = call(
        &mut app,
        "tilemap.create",
        &[("tileset", text(TILESET)), ("name", text("Farm"))],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    assert_eq!(entries(&app), count + 1, "one undo step");

    let map = maps(&mut app)[0];
    let made = tilemap(&app, map);
    assert_eq!(made.cell_size, tileset(&app).tile_size);
    assert_eq!(made.layers.len(), 1);
    assert_eq!(made.layers[0].tileset, TILESET);
    assert_eq!(made.layers[0].kind, TilemapLayerKind::Terrain);
    assert!(made.data_path.ends_with(".jdtiles"), "{}", made.data_path);
    assert_eq!(app.world().get::<Name>(map).map(Name::as_str), Some("Farm"));
    assert_eq!(
        app.world().get::<ChildOf>(map).map(ChildOf::parent),
        Some(root)
    );
    assert_eq!(app.world().resource::<Selection>().primary(), Some(map));
    let key = key_of(&mut app, map);
    assert!(app.world().resource::<TilemapDataStore>().contains(&key));

    let result = call(&mut app, "history.undo", &[]);
    assert_eq!(result, OperatorResult::Finished);
    assert!(maps(&mut app).is_empty(), "undo removes the map");
    assert!(
        !app.world().resource::<TilemapDataStore>().contains(&key),
        "undo drops the map's cells"
    );

    let _ = call(&mut app, "history.redo", &[]);
    let map = maps(&mut app)[0];
    assert_eq!(tilemap(&app, map), made);
    assert_eq!(key_of(&mut app, map), key);
    assert!(app.world().resource::<TilemapDataStore>().contains(&key));
}

#[test]
fn create_takes_the_tile_kind() {
    let (mut app, _dir) = project_app();
    jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    let result = call(
        &mut app,
        "tilemap.create",
        &[("tileset", text(TILESET)), ("kind", text("tile"))],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    let map = maps(&mut app)[0];
    assert_eq!(tilemap(&app, map).layers[0].kind, TilemapLayerKind::Tile);
}

#[test]
fn create_needs_a_2d_scene_and_a_tileset() {
    let (mut app, _dir) = project_app();
    assert!(!available(&mut app, "tilemap.create"));
    assert!(reason(&mut app, "tilemap.create").contains("2D scene"));

    let mut bare = crate::util::editor_test_app();
    jackdaw_editor_core::entity_ops::seed_2d_scene_root(bare.world_mut());
    settle(&mut bare);
    assert!(!available(&mut bare, "tilemap.create"));
    let why = reason(&mut bare, "tilemap.create");
    assert!(why.contains("Import as Tileset"), "{why}");

    jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    assert!(available(&mut app, "tilemap.create"));
}

#[test]
fn removing_a_layer_and_undoing_it_restores_its_cells_exactly() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    assert_eq!(tilemap(&app, map).layers.len(), 2);
    paint(&mut app, map, "0", "terrain", "2");
    paint(&mut app, map, "Props", "tile", "0:3");
    let before = (tilemap(&app, map), stored(&mut app, map));
    let count = entries(&app);

    let result = call(
        &mut app,
        "tilemap.layer.remove",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", text(&format!("#{}", before.0.layers[0].id))),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    assert_eq!(entries(&app), count + 1);
    let after = (tilemap(&app, map), stored(&mut app, map));
    assert_eq!(after.0.layers, before.0.layers[1..]);
    let mut kept = before.1.layers.clone();
    kept.remove(&before.0.layers[0].id);
    assert_eq!(after.1.layers, kept, "the layer above keeps its cells");

    let _ = call(&mut app, "history.undo", &[]);
    assert_eq!((tilemap(&app, map), stored(&mut app, map)), before);
    let _ = call(&mut app, "history.redo", &[]);
    assert_eq!((tilemap(&app, map), stored(&mut app, map)), after);
}

#[test]
fn the_last_layer_is_never_removed() {
    let (mut app, _dir, _root, map) = app_with_map();
    let count = entries(&app);
    let result = call(
        &mut app,
        "tilemap.layer.remove",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", 0i64.into()),
        ],
    );
    assert_eq!(result, OperatorResult::Cancelled);
    assert!(warnings(&app).contains("last layer"), "{}", warnings(&app));
    assert_eq!(tilemap(&app, map).layers.len(), 1);
    assert_eq!(entries(&app), count);
}

#[test]
fn set_changes_a_layer_and_undo_restores_its_cells() {
    let (mut app, _dir, _root, map) = app_with_map();
    paint(&mut app, map, "0", "terrain", "2");
    let before = (tilemap(&app, map), stored(&mut app, map));

    let result = call(
        &mut app,
        "tilemap.layer.set",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", 0i64.into()),
            ("name", text("Ground")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    assert_eq!(tilemap(&app, map).layers[0].name, "Ground");
    assert_eq!(stored(&mut app, map), before.1, "a rename keeps the cells");

    let result = call(
        &mut app,
        "tilemap.layer.set",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", 0i64.into()),
            ("kind", text("tile")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    let layer = tilemap(&app, map).layers[0].clone();
    assert_eq!(layer.kind, TilemapLayerKind::Tile);
    assert_eq!(layer.id, before.0.layers[0].id, "the layer keeps its id");
    assert!(
        (stored(&mut app, map).layers.get(&layer.id)).is_none_or(|cells| cells
            .tiles
            .chunks()
            .next()
            .is_none()),
        "a new kind starts empty"
    );

    let _ = call(&mut app, "history.undo", &[]);
    let _ = call(&mut app, "history.undo", &[]);
    assert_eq!((tilemap(&app, map), stored(&mut app, map)), before);
}

#[test]
fn removing_layers_keeps_the_active_layer_valid() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    add_tile_layer(&mut app, map, "Top");
    let activate = |app: &mut App, index: i64| {
        let result = call(
            app,
            "tilemap.layer.activate",
            &[
                ("entity", PropertyValue::Entity(map)),
                ("layer", index.into()),
            ],
        );
        assert_eq!(result, OperatorResult::Finished, "{}", warnings(app));
    };
    let remove = |app: &mut App, index: i64| {
        let _ = call(
            app,
            "tilemap.layer.remove",
            &[
                ("entity", PropertyValue::Entity(map)),
                ("layer", index.into()),
            ],
        );
    };
    let active = |app: &App| *app.world().resource::<TileEditMode>();

    activate(&mut app, 2);
    assert_eq!((active(&app).map, active(&app).layer), (Some(map), 2));
    remove(&mut app, 0);
    assert_eq!(active(&app).layer, 1, "the same layer stays active");
    let _ = call(&mut app, "history.undo", &[]);
    assert_eq!(active(&app).layer, 2, "undo shifts it back");
}

fn layer_buttons(app: &mut App) -> Vec<(Entity, TilemapLayerButton)> {
    let mut found: Vec<_> = app
        .world_mut()
        .query::<(Entity, &TilemapLayerButton)>()
        .iter(app.world())
        .map(|(entity, button)| (entity, *button))
        .collect();
    found.sort_by_key(|(_, button)| button.index);
    found
}

#[test]
fn the_card_lists_the_layers_and_a_click_paints_one() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    jackdaw_editor_core::selection::select_only(app.world_mut(), map);
    settle(&mut app);

    let buttons = layer_buttons(&mut app);
    assert_eq!(
        buttons.iter().map(|(_, b)| b.index).collect::<Vec<_>>(),
        [0, 1]
    );
    let (props, _) = buttons[1];
    app.world_mut().trigger(Activate { entity: props });
    settle(&mut app);
    let mode = *app.world().resource::<TileEditMode>();
    assert_eq!((mode.map, mode.layer), (Some(map), 1));

    let (first, _) = layer_buttons(&mut app)[0];
    app.world_mut().trigger(Activate { entity: first });
    settle(&mut app);
    assert_eq!(app.world().resource::<TileEditMode>().layer, 0);

    let add = app
        .world_mut()
        .query_filtered::<Entity, With<TilemapAddLayerButton>>()
        .single(app.world())
        .expect("one add button");
    app.world_mut().trigger(Activate { entity: add });
    settle(&mut app);
    assert_eq!(tilemap(&app, map).layers.len(), 3);
    assert_eq!(
        layer_buttons(&mut app).len(),
        3,
        "the card shows the new layer"
    );
}

#[test]
fn the_add_menu_offers_a_tilemap_in_the_2d_group() {
    let (mut app, _dir) = project_app();
    jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    settle(&mut app);
    let taxonomy =
        jackdaw_editor_core::creation_taxonomy::CreationTaxonomy::collect(app.world_mut());
    assert!(
        taxonomy
            .entries_of("2d")
            .any(|entry| entry.label == "Tilemap" && entry.action == "op:tilemap.create")
    );
}

/// Imports the overworld sheet a second time as a plain atlas with no terrain
/// sets, as `plain.tileset.bsn`.
fn import_plain(app: &mut App) {
    let result = call(
        app,
        "tileset.import",
        &[
            ("paths", text("overworld.png")),
            ("out", text("plain")),
            ("setup", text("none")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(app));
}

fn set(app: &mut App, map: Entity, index: i64, param: &'static str, value: &str) -> OperatorResult {
    call(
        app,
        "tilemap.layer.set",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", index.into()),
            (param, text(value)),
        ],
    )
}

fn save_as(app: &mut App, path: &str) {
    jackdaw_editor_core::scene_io::retarget_active_scene(app.world_mut(), path).expect("retarget");
    assert!(
        jackdaw_editor_core::scene_io::save_scene(app.world_mut()),
        "the scene saves"
    );
    settle(app);
}

fn open_card(app: &mut App, map: Entity) {
    jackdaw_editor_core::selection::select_only(app.world_mut(), map);
    settle(app);
}

fn combo_of(app: &mut App, index: usize, param: &str) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &LayerCombo)>()
        .iter(app.world())
        .filter(|(_, combo)| combo.index == index && combo.param == param)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one combo for layer {index}");
    found[0]
}

#[test]
fn undo_and_redo_of_create_reach_the_map_after_save_as() {
    let (mut app, _dir, _root, map) = app_with_map();
    paint(&mut app, map, "0", "terrain", "2");
    let painted = stored(&mut app, map);
    let before = tilemap(&app, map).data_path;
    save_as(&mut app, "farm.bsn");
    assert_ne!(
        tilemap(&app, map).data_path,
        before,
        "the save moved the cells"
    );

    let _ = call(&mut app, "history.undo", &[]);
    let _ = call(&mut app, "history.undo", &[]);
    assert!(maps(&mut app).is_empty());
    let _ = call(&mut app, "history.redo", &[]);
    let _ = call(&mut app, "history.redo", &[]);
    let map = maps(&mut app)[0];
    assert_eq!(stored(&mut app, map), painted, "the cells came back");
}

#[test]
fn the_card_sets_a_terrain_layers_match_mode() {
    let (mut app, _dir, _root, map) = app_with_map();
    assert_eq!(
        set(&mut app, map, 0, "terrain_match", "auto"),
        OperatorResult::Finished
    );
    assert_eq!(
        tilemap(&app, map).layers[0].terrain_match,
        LayerTerrainMatch::Auto
    );
    open_card(&mut app, map);
    let combo = combo_of(&mut app, 0, "terrain_match");
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo,
        selected: 1,
        label: "corners".into(),
        value: None,
    });
    settle(&mut app);
    assert_eq!(
        tilemap(&app, map).layers[0].terrain_match,
        LayerTerrainMatch::Corners,
        "the mixed-mode tileset gets a mode from the card"
    );
}

#[test]
fn the_card_renames_a_layer() {
    let (mut app, _dir, _root, map) = app_with_map();
    open_card(&mut app, map);
    let field = app
        .world_mut()
        .query_filtered::<Entity, With<LayerNameField>>()
        .single(app.world())
        .expect("one name field");
    app.world_mut().trigger(TextEditCommitEvent {
        entity: field,
        text: "Ground".into(),
    });
    settle(&mut app);
    assert_eq!(tilemap(&app, map).layers[0].name, "Ground");
}

#[test]
fn a_new_tileset_empties_the_layer_in_the_same_undo_step() {
    let (mut app, _dir, _root, map) = app_with_map();
    import_plain(&mut app);
    add_tile_layer(&mut app, map, "Props");
    paint(&mut app, map, "Props", "tile", "0:3");
    let before = (tilemap(&app, map), stored(&mut app, map));
    let count = entries(&app);

    assert_eq!(
        set(&mut app, map, 1, "tileset", "plain.tileset.bsn"),
        OperatorResult::Finished
    );
    assert_eq!(entries(&app), count + 1);
    let layer = tilemap(&app, map).layers[1].clone();
    assert_eq!(layer.tileset, "plain.tileset.bsn");
    assert!(
        (stored(&mut app, map).layers.get(&layer.id)).is_none_or(|cells| cells
            .tiles
            .chunks()
            .next()
            .is_none()),
        "the old tileset's indices are gone"
    );
    let _ = call(&mut app, "history.undo", &[]);
    assert_eq!((tilemap(&app, map), stored(&mut app, map)), before);
}

#[test]
fn a_refused_card_choice_shows_the_layer_again() {
    let (mut app, _dir, _root, map) = app_with_map();
    import_plain(&mut app);
    let result = call(
        &mut app,
        "tilemap.layer.add",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("tileset", text("plain.tileset.bsn")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    assert_eq!(tilemap(&app, map).layers[1].kind, TilemapLayerKind::Tile);
    open_card(&mut app, map);
    let combo = combo_of(&mut app, 1, "kind");
    app.world_mut()
        .entity_mut(combo)
        .insert(ComboBoxSelectedIndex(0));
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo,
        selected: 0,
        label: "terrain".into(),
        value: None,
    });
    settle(&mut app);
    assert_eq!(
        tilemap(&app, map).layers[1].kind,
        TilemapLayerKind::Tile,
        "refused"
    );
    let combo = combo_of(&mut app, 1, "kind");
    assert_eq!(
        app.world().get::<ComboBoxSelectedIndex>(combo).map(|s| s.0),
        Some(1),
        "the combo shows the tile kind the layer kept"
    );
}

#[test]
fn layer_edits_wait_for_a_stroke_to_end() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    crate::stage::tile_tools::enter(&mut app, map);
    crate::stage::tile_tools::hold(&mut app, "tilemap.brush", MouseButton::Left, &[(0, 0)]);
    for id in [
        "tilemap.layer.add",
        "tilemap.layer.remove",
        "tilemap.layer.set",
    ] {
        assert!(!available(&mut app, id), "{id} waits for the stroke");
    }
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    assert!(available(&mut app, "tilemap.layer.add"));
}

#[test]
fn undoing_an_add_the_cells_never_held_leaves_them_saved() {
    let (mut app, _dir, _root, map) = app_with_map();
    save_as(&mut app, "farm.bsn");
    let key = key_of(&mut app, map);
    let unsaved = |app: &App| {
        app.world()
            .resource::<TilemapDataStore>()
            .has_unsaved_edits(&key)
    };
    assert!(!unsaved(&app));
    add_tile_layer(&mut app, map, "Props");
    let _ = call(&mut app, "history.undo", &[]);
    assert!(!unsaved(&app), "nothing of the cells changed");
}

#[test]
fn removing_the_active_layer_leaves_tile_edit_mode() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    let result = call(
        &mut app,
        "tilemap.layer.activate",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", 1i64.into()),
        ],
    );
    assert_eq!(result, OperatorResult::Finished);
    let _ = call(
        &mut app,
        "tilemap.layer.remove",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", 1i64.into()),
        ],
    );
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
}

#[test]
fn reordering_the_layer_list_keeps_every_layers_cells() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    paint(&mut app, map, "0", "terrain", "2");
    paint(&mut app, map, "Props", "tile", "0:3");
    let before = stored(&mut app, map);
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("a tilemap")
        .layers
        .reverse();
    settle(&mut app);
    assert_eq!(stored(&mut app, map), before, "no cell moved");

    let props = tilemap(&app, map).layers[0].id;
    let result = call(
        &mut app,
        "tilemap.set_cells",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", text("0")),
            ("kind", text("tile")),
            ("cells", text("9,9")),
            ("value", text("0:3")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    let after = stored(&mut app, map);
    let painted = |data: &TilemapData, id| data.layers[&id].painted_cells().len();
    assert_eq!(painted(&after, props), painted(&before, props) + 1);
    let ground = tilemap(&app, map).layers[1].id;
    assert_eq!(after.layers[&ground], before.layers[&ground]);
}

#[test]
fn layers_are_named_by_id_and_ids_are_never_given_twice() {
    let (mut app, _dir, _root, map) = app_with_map();
    add_tile_layer(&mut app, map, "Props");
    let props = tilemap(&app, map).layers[1].id;
    let _ = call(&mut app, "history.undo", &[]);
    add_tile_layer(&mut app, map, "Decor");
    let decor = tilemap(&app, map).layers[1].id;
    assert_ne!(decor, props, "an undone layer's id stays unused");

    let result = call(
        &mut app,
        "tilemap.layer.set",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", text(&format!("#{decor}"))),
            ("name", text("Trees")),
        ],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    assert_eq!(tilemap(&app, map).layers[1].name, "Trees");
    let result = call(
        &mut app,
        "tilemap.layer.remove",
        &[
            ("entity", PropertyValue::Entity(map)),
            ("layer", text("#999")),
        ],
    );
    assert_eq!(result, OperatorResult::Cancelled);
    assert!(warnings(&app).contains("#999"), "{}", warnings(&app));
}

#[test]
fn a_layer_repeating_another_layers_id_gets_a_fresh_one() {
    let (mut app, _dir, _root, map) = app_with_map();
    paint(&mut app, map, "0", "terrain", "2");
    let before = stored(&mut app, map);
    let mut copy = tilemap(&app, map).layers[0].clone();
    copy.name = "Copy".into();
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("a tilemap")
        .layers
        .push(copy);
    settle(&mut app);
    let layers = tilemap(&app, map).layers;
    assert_ne!(layers[0].id, layers[1].id);
    assert_eq!(
        stored(&mut app, map),
        before,
        "the cells stay with the first"
    );
}
