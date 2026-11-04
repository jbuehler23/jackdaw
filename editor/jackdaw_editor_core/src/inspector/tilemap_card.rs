//! The `Tilemap` card: an Edit tiles button, one row per layer with its name,
//! kind, tileset and match mode, then the map's cell size, bounds and data
//! file. Every control calls an operator.

use bevy::{ecs::system::SystemState, picking::hover::Hovered, prelude::*};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{
        ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, IconButtonProps, button,
        icon_button,
    },
    combobox::{
        ComboBoxChangeEvent, ComboBoxOptionData, ComboBoxSelectedIndex, combobox_with_selected,
    },
    field_row::{FieldRowProps, spawn_field_row},
    icons::{EditorFont, Icon, IconFont},
    text_edit::{TextEditCommitEvent, TextEditProps, text_edit},
    tokens,
    tooltip::Tooltip,
    utils::find_ancestor,
};
use jackdaw_scene_types::{Tilemap, TilemapLayer, TilemapLayerKind};
use jackdaw_tileset::Tileset;

use crate::tile_tools::{TileEditMode, TilemapEditModeExitOp, TilemapEditModeOp};
use crate::tilemap_ops::{
    MATCHES, TilemapLayerActivateOp, TilemapLayerAddOp, TilemapLayerRemoveOp, TilemapLayerSetOp,
    project_tilesets,
};
use crate::tileset_dock::TilesetEditOp;

/// The button that paints layer `index` of `map`.
#[derive(Component, Clone, Copy, Debug)]
pub struct TilemapLayerButton {
    pub map: Entity,
    pub index: usize,
}

/// The card's button that enters tile edit mode on `map`, or with `exit` the
/// one that leaves it; the card shows one of the two.
#[derive(Component, Clone, Copy, Debug)]
pub struct TilemapEditButton {
    pub map: Entity,
    pub exit: bool,
}

/// The row of layer `index` of `map`.
#[derive(Component, Clone, Copy, Debug)]
pub struct TilemapLayerRow {
    pub map: Entity,
    pub index: usize,
}

/// The hint a layer row shows and the tileset it read it from, kept to notice
/// when that tileset changes it.
#[derive(Component)]
struct LayerHint {
    shown: Option<&'static str>,
    tileset: Option<AssetId<Tileset>>,
}

/// The button that opens the tileset of a terrain layer with nothing to paint.
#[derive(Component)]
pub struct TerrainHintButton;

/// The button that adds a layer to the card's map.
#[derive(Component)]
pub struct TilemapAddLayerButton;

/// A combo that sets `param` of layer `index` to the value of the option
/// picked.
#[derive(Component)]
pub struct LayerCombo {
    pub map: Entity,
    pub index: usize,
    pub param: &'static str,
    values: Vec<String>,
}

/// The name field of layer `index`.
#[derive(Component)]
pub struct LayerNameField {
    pub map: Entity,
    pub index: usize,
}

const NO_SETS: &str = "This layer paints terrain, but its tileset has no terrain sets yet. Open \
                       the tileset to add one.";
const NO_BITS: &str = "This layer paints terrain, but no tile of its tileset has terrain bits \
                       yet. Open the tileset to paint them.";

/// Width of a layer's kind combo, so every row's tileset starts at one x.
const KIND_WIDTH: f32 = 76.0;
/// Room a terrain layer's match combo keeps before it wraps.
const MATCH_MIN: f32 = 80.0;

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(on_combo_pick)
        .add_observer(on_name_commit)
        .add_systems(
            Update,
            (
                show_active_layer.run_if(resource_changed::<TileEditMode>),
                refresh_layer_hints,
            ),
        );
}

fn painting(mode: &TileEditMode, map: Entity, index: usize) -> bool {
    mode.map == Some(map) && mode.layer == index
}

fn row_background(active: bool) -> BackgroundColor {
    BackgroundColor(if active {
        tokens::SELECTED_BG
    } else {
        Color::NONE
    })
}

fn marker_variant(active: bool) -> ButtonVariant {
    if active {
        ButtonVariant::Active
    } else {
        ButtonVariant::Ghost
    }
}

/// The hint of a terrain layer and the tileset it reads; no hint for a tile
/// layer.
fn layer_hint(world: &mut World, layer: &TilemapLayer) -> LayerHint {
    let tileset = (layer.kind == TilemapLayerKind::Terrain)
        .then(|| crate::tileset_dock::tileset_id(world, &layer.tileset).ok())
        .flatten();
    let shown = tileset.and_then(|id| hint_of(world.resource::<Assets<Tileset>>().get(id)));
    LayerHint { shown, tileset }
}

/// Why a terrain layer of `tileset` paints nothing: no terrain sets, or no tile
/// with terrain bits. `None` while the tileset is not loaded.
fn hint_of(tileset: Option<&Tileset>) -> Option<&'static str> {
    let tileset = tileset?;
    if tileset.terrain_sets.is_empty() {
        Some(NO_SETS)
    } else if !(tileset.tiles.iter()).any(|tile| tile.peers.iter().any(|peer| *peer != 0)) {
        Some(NO_BITS)
    } else {
        None
    }
}

/// Fill the `Tilemap` card body for `source`. World-exclusive and deferred by
/// the caller.
pub(crate) fn fill_tilemap_card_body(world: &mut World, source: Entity, body: Entity) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(tilemap) = world.get::<Tilemap>(source).cloned() else {
        return;
    };
    let mode = *world.resource::<TileEditMode>();
    let tilesets = project_tilesets(world);
    let hints: Vec<_> = (tilemap.layers.iter())
        .map(|layer| layer_hint(world, layer))
        .collect();
    let icon_font = (world.get_resource::<IconFont>()).map_or_else(default, |f| f.0.clone());
    let editor_font = (world.get_resource::<EditorFont>()).map_or_else(default, |f| f.0.clone());
    let registry = (world.get_resource::<AppTypeRegistry>())
        .cloned()
        .unwrap_or_default();

    let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
    if let Ok((mut commands, names)) = state.get_mut(world) {
        let editing = mode.map == Some(source);
        for exit in [false, true] {
            let (caption, icon, op) = if exit {
                ("Exit tile editing", Icon::Check, TilemapEditModeExitOp::ID)
            } else {
                ("Edit tiles", Icon::Grid2x2, TilemapEditModeOp::ID)
            };
            let mut props = ButtonProps::new(caption).with_left_icon(icon);
            if exit {
                props = props.with_variant(ButtonVariant::Active);
            }
            if exit != editing {
                props = props.hidden();
            }
            commands.spawn((
                button(props),
                TilemapEditButton { map: source, exit },
                if exit {
                    ButtonOperatorCall::new(op)
                } else {
                    ButtonOperatorCall::new(op).with_param("entity", source)
                },
                ChildOf(body),
            ));
        }

        commands.spawn((
            Text::new("Layers"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                margin: UiRect::vertical(px(tokens::SPACING_SM)),
                ..default()
            },
            ChildOf(body),
        ));
        for (index, (layer, hint)) in tilemap.layers.iter().zip(hints).enumerate() {
            let layer_row = TilemapLayerRow { map: source, index };
            let active = painting(&mode, source, index);
            let row = commands
                .spawn((
                    Node {
                        align_items: AlignItems::Start,
                        column_gap: px(tokens::SPACING_SM),
                        width: percent(100.0),
                        padding: UiRect::all(px(tokens::SPACING_XS)),
                        border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                        ..default()
                    },
                    row_background(active),
                    layer_row,
                    ChildOf(body),
                ))
                .id();
            let shown = hint.shown;
            commands.entity(row).insert(hint);
            let content = spawn_layer_row(
                &mut commands,
                row,
                layer,
                layer_row,
                active,
                &tilesets,
                &icon_font,
            );
            if let Some(hint) = shown {
                spawn_hint(&mut commands, content, hint, &layer.tileset);
            }
        }
        commands.spawn((
            button(ButtonProps::new("Add layer").with_left_icon(Icon::Plus)),
            TilemapAddLayerButton,
            ButtonOperatorCall::new(TilemapLayerAddOp::ID).with_param("entity", source),
            ChildOf(body),
        ));

        let type_path = <Tilemap as bevy::reflect::TypePath>::type_path();
        for (field, label) in [("cell_size", "Cell size"), ("bounds", "Bounds")] {
            if let Some(value) = tilemap.field(field) {
                super::reflect_fields::spawn_field_row(
                    &mut commands,
                    body,
                    label,
                    value,
                    0,
                    field.to_string(),
                    source,
                    type_path,
                    &names,
                    &registry,
                    &editor_font,
                    &icon_font,
                );
            }
        }
        let row = spawn_field_row(&mut commands, body, FieldRowProps::new("Data file"));
        super::reflect_fields::spawn_string_input(
            &mut commands,
            row.control,
            &tilemap.data_path,
            "data_path".to_string(),
            source,
            type_path,
        );
    }
    state.apply(world);
    world.flush();
}

/// A layer's paint marker beside three lines: its name and remove button; its
/// kind and, for a terrain layer, match mode; its tileset. Returns the column
/// holding the lines.
fn spawn_layer_row(
    commands: &mut Commands,
    row: Entity,
    layer: &TilemapLayer,
    layer_row: TilemapLayerRow,
    active: bool,
    tilesets: &[String],
    icon_font: &Handle<Font>,
) -> Entity {
    let TilemapLayerRow { map, index } = layer_row;
    let marker = commands
        .spawn((
            Node {
                min_height: px(tokens::ROW_HEIGHT),
                align_items: AlignItems::Center,
                ..default()
            },
            ChildOf(row),
        ))
        .id();
    let lines = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                flex_grow: 1.0,
                min_width: px(0.0),
                ..default()
            },
            ChildOf(row),
        ))
        .id();
    let line = |commands: &mut Commands| {
        commands
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(tokens::SPACING_SM),
                    row_gap: px(tokens::SPACING_XS),
                    width: percent(100.0),
                    ..default()
                },
                ChildOf(lines),
            ))
            .id()
    };
    let call = |id: &'static str| {
        ButtonOperatorCall::new(id)
            .with_param("entity", map)
            .with_param("index", index as i64)
    };
    let combo = |param: &'static str, values: Vec<String>| LayerCombo {
        map,
        index,
        param,
        values,
    };
    let icon = |icon: Icon, variant: ButtonVariant| {
        let props = IconButtonProps::new(icon).with_size(ButtonSize::IconSM);
        icon_button(props.variant(variant), icon_font)
    };

    commands.spawn((
        icon(Icon::Paintbrush, marker_variant(active)),
        TilemapLayerButton { map, index },
        call(TilemapLayerActivateOp::ID),
        ChildOf(marker),
    ));
    let head = line(commands);
    let name = holder(commands, head, Slot::Grow(0.0));
    commands.spawn((
        text_edit(TextEditProps::default().with_default_value(layer.name.clone())),
        LayerNameField { map, index },
        ChildOf(name),
    ));
    commands.spawn((
        icon(Icon::Trash2, ButtonVariant::Ghost),
        call(TilemapLayerRemoveOp::ID),
        ChildOf(head),
    ));

    let tail = line(commands);
    let terrain = layer.kind == TilemapLayerKind::Terrain;
    let kinds = vec!["terrain".to_string(), "tile".to_string()];
    let kind = holder(commands, tail, Slot::Fixed(KIND_WIDTH));
    spawn_combo(
        commands,
        kind,
        combo("kind", kinds),
        usize::from(!terrain),
        None,
    );
    if terrain {
        let values = MATCHES.iter().map(|(name, _)| name.to_string()).collect();
        let picked = MATCHES.iter().position(|(_, m)| *m == layer.terrain_match);
        let tip = Tooltip::title("Terrain match")
            .with_description("Which neighbours pick a terrain tile; auto follows the tileset.");
        let matches = combo("terrain_match", values);
        let slot = holder(commands, tail, Slot::Grow(MATCH_MIN));
        spawn_combo(commands, slot, matches, picked.unwrap_or(0), Some(tip));
    }
    let mut paths = tilesets.to_vec();
    if !paths.contains(&layer.tileset) {
        paths.push(layer.tileset.clone());
    }
    let picked = paths.iter().position(|p| *p == layer.tileset).unwrap_or(0);
    let last = line(commands);
    let tileset = holder(commands, last, Slot::Grow(0.0));
    let tip = Tooltip::title(layer.tileset.clone()).with_description("The layer's tileset");
    spawn_combo(
        commands,
        tileset,
        combo("tileset", paths),
        picked,
        Some(tip),
    );
    lines
}

/// How a control takes room in a layer line.
enum Slot {
    /// This wide, or the whole line when the line is narrower.
    Fixed(f32),
    /// The room left over, wrapping to its own line below this much.
    Grow(f32),
}

fn holder(commands: &mut Commands, line: Entity, slot: Slot) -> Entity {
    let node = match slot {
        Slot::Fixed(width) => Node {
            width: px(width),
            max_width: percent(100.0),
            ..default()
        },
        Slot::Grow(basis) => Node {
            flex_grow: 1.0,
            flex_basis: px(basis),
            min_width: px(0.0),
            ..default()
        },
    };
    commands.spawn((node, ChildOf(line))).id()
}

fn spawn_combo(
    commands: &mut Commands,
    parent: Entity,
    combo: LayerCombo,
    picked: usize,
    tip: Option<Tooltip>,
) {
    let labels = combo.values.iter().map(|value| match combo.param {
        "tileset" => crate::tileset_import::tileset_label(value),
        _ => super::sentence(value),
    });
    let options: Vec<ComboBoxOptionData> = labels.map(ComboBoxOptionData::new).collect();
    let mut spawned = commands.spawn((
        combobox_with_selected(options, picked),
        ComboBoxSelectedIndex(picked),
        combo,
        ChildOf(parent),
    ));
    if let Some(tip) = tip {
        spawned.insert((tip, Hovered::default()));
    }
}

/// Why the layer paints nothing, with a button that opens its tileset.
fn spawn_hint(commands: &mut Commands, lines: Entity, hint: &str, tileset: &str) {
    let line = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Start,
                row_gap: px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(lines),
        ))
        .id();
    commands.spawn((
        Text::new(hint),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(line),
    ));
    commands.spawn((
        button(ButtonProps::new("Open tileset").with_left_icon(Icon::FolderOpen)),
        TerrainHintButton,
        ButtonOperatorCall::new(TilesetEditOp::ID).with_param("path", tileset.to_string()),
        ChildOf(line),
    ));
}

fn set_layer(
    commands: &mut Commands,
    map: Entity,
    index: usize,
    param: &'static str,
    value: String,
) {
    commands
        .operator(TilemapLayerSetOp::ID)
        .param("entity", map)
        .param("index", index as i64)
        .param(param, value)
        .call();
}

fn on_combo_pick(
    pick: On<ComboBoxChangeEvent>,
    combos: Query<&LayerCombo>,
    mut commands: Commands,
) {
    let Ok(combo) = combos.get(pick.entity) else {
        return;
    };
    if let Some(value) = combo.values.get(pick.selected) {
        set_layer(
            &mut commands,
            combo.map,
            combo.index,
            combo.param,
            value.clone(),
        );
    }
}

fn on_name_commit(
    commit: On<TextEditCommitEvent>,
    fields: Query<&LayerNameField>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let name = commit.text.trim().to_string();
    if let Some((_, field)) = find_ancestor(commit.entity, &fields, &parents)
        && !name.is_empty()
    {
        set_layer(&mut commands, field.map, field.index, "name", name);
    }
}

/// Mark the layer tile edit mode paints, and offer to leave the mode on the
/// map it is on.
fn show_active_layer(
    mode: Res<TileEditMode>,
    mut markers: Query<(&TilemapLayerButton, &mut ButtonVariant)>,
    mut rows: Query<(&TilemapLayerRow, &mut BackgroundColor)>,
    mut edits: Query<(&TilemapEditButton, &mut Node)>,
) {
    for (marker, mut variant) in &mut markers {
        variant.set_if_neq(marker_variant(painting(&mode, marker.map, marker.index)));
    }
    for (row, mut background) in &mut rows {
        background.set_if_neq(row_background(painting(&mode, row.map, row.index)));
    }
    for (button, mut node) in &mut edits {
        let shown = button.exit == (mode.map == Some(button.map));
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}

/// Rebuild a card whose layer hint a tileset change settled or raised.
fn refresh_layer_hints(
    mut events: MessageReader<AssetEvent<Tileset>>,
    tilesets: Res<Assets<Tileset>>,
    rows: Query<(&TilemapLayerRow, &LayerHint)>,
    mut rebuild: ResMut<super::PendingInspectorRebuild>,
) {
    let changed: Vec<AssetId<Tileset>> = (events.read())
        .filter_map(|event| match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            _ => None,
        })
        .collect();
    for (row, hint) in &rows {
        if let Some(id) = hint.tileset
            && changed.contains(&id)
            && hint_of(tilesets.get(id)) != hint.shown
        {
            rebuild.0 = Some(row.map);
            return;
        }
    }
}
