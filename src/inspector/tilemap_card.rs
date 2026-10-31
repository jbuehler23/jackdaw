//! The `Tilemap` card: its Layers section, each layer a button that paints it
//! with its name, kind, tileset and match mode below, then the map's other
//! fields. Every control calls an operator.

use bevy::{ecs::system::SystemState, prelude::*};
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
    utils::find_ancestor,
};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};

use crate::tile_tools::TileEditMode;
use crate::tilemap_ops::{
    MATCHES, TilemapLayerActivateOp, TilemapLayerAddOp, TilemapLayerRemoveOp, TilemapLayerSetOp,
    project_tilesets,
};

/// The button that paints layer `index` of `map`.
#[derive(Component, Clone, Copy, Debug)]
pub struct TilemapLayerButton {
    pub map: Entity,
    pub index: usize,
}

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

/// The map fields shown below the layers.
const FIELDS: [&str; 3] = ["cell_size", "bounds", "data_path"];

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(on_combo_pick)
        .add_observer(on_name_commit)
        .add_systems(
            Update,
            show_active_layer.run_if(resource_changed::<TileEditMode>),
        );
}

fn layer_variant(mode: &TileEditMode, button: &TilemapLayerButton) -> ButtonVariant {
    if mode.map == Some(button.map) && mode.layer == button.index {
        ButtonVariant::Active
    } else {
        ButtonVariant::Ghost
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
    let icon_font = (world.get_resource::<IconFont>()).map_or_else(default, |f| f.0.clone());
    let editor_font = (world.get_resource::<EditorFont>()).map_or_else(default, |f| f.0.clone());
    let registry = (world.get_resource::<AppTypeRegistry>())
        .cloned()
        .unwrap_or_default();

    let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
    if let Ok((mut commands, names)) = state.get_mut(world) {
        commands.spawn((
            Text::new("Layers"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(body),
        ));
        for (index, layer) in tilemap.layers.iter().enumerate() {
            let head = commands
                .spawn((
                    Node {
                        column_gap: px(tokens::SPACING_XS),
                        width: percent(100.0),
                        ..default()
                    },
                    ChildOf(body),
                ))
                .id();
            let marker = TilemapLayerButton { map: source, index };
            let call = |id: &'static str| {
                ButtonOperatorCall::new(id)
                    .with_param("entity", source)
                    .with_param("index", index as i64)
            };
            let paint = commands
                .spawn((
                    button(
                        ButtonProps::new(layer.name.clone())
                            .with_variant(layer_variant(&mode, &marker))
                            .with_size(ButtonSize::MD)
                            .align_left(),
                    ),
                    marker,
                    call(TilemapLayerActivateOp::ID),
                    ChildOf(head),
                ))
                .id();
            commands
                .entity(paint)
                .entry::<Node>()
                .and_modify(|mut node| node.flex_grow = 1.0);
            commands.spawn((
                icon_button(IconButtonProps::new(Icon::Trash2), &icon_font),
                call(TilemapLayerRemoveOp::ID),
                ChildOf(head),
            ));

            let row = spawn_field_row(&mut commands, body, FieldRowProps::new("name").indented(1));
            commands.spawn((
                text_edit(TextEditProps::default().with_default_value(layer.name.clone())),
                LayerNameField { map: source, index },
                ChildOf(row.control),
            ));

            let combo = |param: &'static str, values: Vec<String>| LayerCombo {
                map: source,
                index,
                param,
                values,
            };
            let kinds = vec!["terrain".to_string(), "tile".to_string()];
            let terrain = layer.kind == TilemapLayerKind::Terrain;
            let labels = kinds.clone();
            let picked = usize::from(!terrain);
            spawn_combo(&mut commands, body, combo("kind", kinds), labels, picked);

            let mut paths = tilesets.clone();
            if !paths.contains(&layer.tileset) {
                paths.push(layer.tileset.clone());
            }
            let picked = paths.iter().position(|p| *p == layer.tileset);
            let labels = paths.iter().map(|path| tileset_label(path)).collect();
            let tileset = combo("tileset", paths);
            spawn_combo(&mut commands, body, tileset, labels, picked.unwrap_or(0));

            if terrain {
                let values: Vec<String> =
                    MATCHES.iter().map(|(name, _)| name.to_string()).collect();
                let labels = values.iter().map(|value| value.replace('_', " ")).collect();
                let picked = MATCHES.iter().position(|(_, m)| *m == layer.terrain_match);
                let matches = combo("terrain_match", values);
                spawn_combo(&mut commands, body, matches, labels, picked.unwrap_or(0));
            }
        }
        commands.spawn((
            button(ButtonProps::new("Add Layer").with_left_icon(Icon::Plus)),
            TilemapAddLayerButton,
            ButtonOperatorCall::new(TilemapLayerAddOp::ID).with_param("entity", source),
            ChildOf(body),
        ));

        let type_path = <Tilemap as bevy::reflect::TypePath>::type_path();
        for field in FIELDS {
            if let Some(value) = tilemap.field(field) {
                super::reflect_fields::spawn_field_row(
                    &mut commands,
                    body,
                    field,
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
    }
    state.apply(world);
    world.flush();
}

fn spawn_combo(
    commands: &mut Commands,
    body: Entity,
    combo: LayerCombo,
    labels: Vec<String>,
    picked: usize,
) {
    let label = combo.param.replace('_', " ");
    let row = spawn_field_row(commands, body, FieldRowProps::new(label).indented(1));
    let options: Vec<ComboBoxOptionData> =
        labels.into_iter().map(ComboBoxOptionData::new).collect();
    commands.spawn((
        combobox_with_selected(options, picked),
        ComboBoxSelectedIndex(picked),
        combo,
        ChildOf(row.control),
    ));
}

/// A tileset path as the combo shows it: its file name less the extension.
fn tileset_label(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(crate::tileset_import::TILESET_SUFFIX)
        .unwrap_or(file)
        .to_string()
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

/// Mark the layer tile edit mode paints.
fn show_active_layer(
    mode: Res<TileEditMode>,
    mut buttons: Query<(&TilemapLayerButton, &mut ButtonVariant)>,
) {
    for (button, mut variant) in &mut buttons {
        variant.set_if_neq(layer_variant(&mode, button));
    }
}
